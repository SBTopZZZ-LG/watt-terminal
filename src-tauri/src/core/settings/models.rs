//! Model lists: reading a provider's `/models` response, fetching it, and merging the result into the saved models.
//!
//! `parse_model_list` is pure and tolerant of the shapes providers use. `fetch_models` makes one GET with the same
//! header rules as the chat adapter (core/llm): a bearer token only when there is a key, and caller headers replace
//! same-named ones. The body is read up to `MAX_BODY_BYTES`, then the connection is dropped. Errors never carry the
//! key or a header value.

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::time::Duration;

use futures_util::StreamExt;
use serde_json::Value;

use super::{MAX_MODEL_ID_CHARS, MAX_VARIANTS_PER_MODEL, Model, ModelSource, ModelVariant, Provider, VariantSource, valid_variant_id};
use crate::core::chat::redact::Redactor;
use crate::core::llm::{self, LlmError, ProviderConfig};

/// Total time for one model-list request, including the body.
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(20);
/// Largest model-list body that is read. A larger one is refused.
pub const MAX_BODY_BYTES: usize = 5 * 1024 * 1024;
/// Most models kept from one list.
const MAX_MODELS: usize = 2000;
/// Largest context size taken from a list, in tokens. Bigger values are treated as bad data.
const MAX_CONTEXT_TOKENS: u64 = 100_000_000;
/// Longest error message kept, in characters.
const MESSAGE_LIMIT: usize = 300;

/// One model from a provider's list. `context` is the size the provider reported, if it reported one. `vision` says
/// whether the provider lists image input among the model's input modalities: Some(true) with "image" in
/// `architecture.input_modalities`, Some(false) when that list is there without it, None when the entry has no list.
/// `variants` are the auto effort variants read from the entry (see `variants_of`), empty when it lists none.
#[derive(Debug, Clone, PartialEq)]
pub struct FetchedModel {
    pub id: String,
    pub context: Option<u32>,
    pub vision: Option<bool>,
    pub variants: Vec<ModelVariant>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelsError {
    /// The provider answered with a non-2xx status. `message` is the provider's text, truncated, with the key removed.
    Http { status: u16, message: String },
    /// The request failed before a usable answer arrived (connection, timeout, bad header).
    Transport(String),
    /// The body was longer than `MAX_BODY_BYTES`.
    TooLarge,
    /// The body is not a model list. The body text is not shown.
    Parse(String),
}

impl fmt::Display for ModelsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ModelsError::Http { status, message } => {
                write!(f, "the model list request failed with HTTP {status}: {message}")
            }
            ModelsError::Transport(detail) => write!(f, "could not load the model list: {detail}"),
            ModelsError::TooLarge => write!(f, "the model list is larger than {MAX_BODY_BYTES} bytes"),
            ModelsError::Parse(detail) => write!(f, "the model list could not be read: {detail}"),
        }
    }
}

impl std::error::Error for ModelsError {}

/// Reads a model list in any of the shapes seen in practice: `{"data": [...]}` (OpenAI, OpenRouter, OpenCode Go),
/// `{"models": [...]}`, or a bare array. An entry is an object with `id` (or `name`, then `model`) or a plain string.
/// The context size is the first valid value among `context_length`, `top_provider.context_length`, `limit.context`,
/// `context_window`, `max_context_length` and `context`. Entries with an unusable id are skipped, and so are repeats
/// (the first one wins). At most `MAX_MODELS` are kept.
pub fn parse_model_list(body: &str) -> Result<Vec<FetchedModel>, ModelsError> {
    // Providers sometimes lead the body with blank lines or spaces (seen on OpenRouter).
    let root: Value =
        serde_json::from_str(body.trim_start()).map_err(|_| ModelsError::Parse("the response is not JSON".into()))?;
    let list: &[Value] = match &root {
        Value::Array(items) => items,
        Value::Object(_) => root
            .get("data")
            .and_then(Value::as_array)
            .or_else(|| root.get("models").and_then(Value::as_array))
            .ok_or_else(|| ModelsError::Parse("the response has no model list".into()))?,
        _ => return Err(ModelsError::Parse("the response has no model list".into())),
    };

    let mut seen = HashSet::new();
    let mut models = Vec::new();
    for entry in list {
        if models.len() == MAX_MODELS {
            break;
        }
        let Some(id) = id_of(entry).filter(|id| usable_id(id)) else {
            continue;
        };
        if !seen.insert(id.clone()) {
            continue;
        }
        models.push(FetchedModel {
            context: context_of(entry),
            vision: vision_of(entry),
            variants: variants_of(entry),
            id,
        });
    }
    Ok(models)
}

/// The id of one entry: a plain string, or the first of `id`, `name`, `model` that is a non-blank string.
fn id_of(entry: &Value) -> Option<String> {
    match entry {
        Value::String(text) => Some(text.clone()),
        Value::Object(_) => ["id", "name", "model"]
            .iter()
            .find_map(|key| entry.get(key)?.as_str().filter(|s| !s.trim().is_empty()).map(str::to_string)),
        _ => None,
    }
}

/// The same rule settings validation applies to a model id, so every fetched id can be saved.
fn usable_id(id: &str) -> bool {
    !id.trim().is_empty() && id.chars().count() <= MAX_MODEL_ID_CHARS && !id.chars().any(char::is_control)
}

/// The first context size that is a positive whole number in range. Floats, strings, zero and absurd values are
/// skipped, and the next known field is tried.
fn context_of(entry: &Value) -> Option<u32> {
    const PATHS: [&[&str]; 6] = [
        &["context_length"],
        &["top_provider", "context_length"],
        &["limit", "context"],
        &["context_window"],
        &["max_context_length"],
        &["context"],
    ];
    PATHS.iter().find_map(|path| {
        let mut value = entry;
        for key in *path {
            value = value.get(key)?;
        }
        let n = value.as_u64().filter(|n| (1..=MAX_CONTEXT_TOKENS).contains(n))?;
        u32::try_from(n).ok()
    })
}

/// Whether the entry takes images, from `architecture.input_modalities` (OpenRouter). None when the entry has no such
/// array of strings; otherwise true only when the array has "image".
fn vision_of(entry: &Value) -> Option<bool> {
    let modalities = entry.get("architecture")?.get("input_modalities")?.as_array()?;
    Some(modalities.iter().any(|m| m.as_str() == Some("image")))
}

/// Effort names that describe the list rather than a selectable effort. Never become variants.
const SKIPPED_EFFORTS: &[&str] = &["toggle", "budget", "default_effort", "mandatory"];

/// The auto effort variants of one entry, built only from validated scalars. OpenRouter lists its efforts at
/// `reasoning.supported_efforts` (body `{"reasoning":{"effort":e}}`); OpenCode/Zen lists them at
/// `capabilities.reasoning.effort` (body `{"reasoning_effort":e}`). When `reasoning.supported_efforts` is an array it
/// wins, whatever else the entry holds; otherwise the OpenCode path is tried. Names are lowercased, validated like a
/// variant id, and deduplicated first-wins; at most `MAX_VARIANTS_PER_MODEL` are kept. All are `VariantSource::Auto`.
fn variants_of(entry: &Value) -> Vec<ModelVariant> {
    let openrouter = entry.get("reasoning").and_then(|r| r.get("supported_efforts")).and_then(Value::as_array);
    let efforts: &[Value] = match openrouter {
        Some(list) => list,
        None => entry
            .get("capabilities")
            .and_then(|c| c.get("reasoning"))
            .and_then(|r| r.get("effort"))
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]),
    };
    let nested = openrouter.is_some();
    let mut seen = HashSet::new();
    let mut variants = Vec::new();
    for effort in efforts {
        if variants.len() == MAX_VARIANTS_PER_MODEL {
            break;
        }
        let Some(raw) = effort.as_str() else { continue };
        let id = raw.to_ascii_lowercase();
        if !valid_variant_id(&id) || SKIPPED_EFFORTS.contains(&id.as_str()) || !seen.insert(id.clone()) {
            continue;
        }
        let body = if nested {
            serde_json::json!({"reasoning": {"effort": id}})
        } else {
            serde_json::json!({"reasoning_effort": id})
        };
        let Value::Object(body) = body else { continue };
        variants.push(ModelVariant { id: id.clone(), label: effort_label(&id), body, source: VariantSource::Auto });
    }
    variants
}

/// The display name of an effort id: title-cased words, with `xhigh` as "X High".
fn effort_label(id: &str) -> String {
    if id == "xhigh" {
        return "X High".to_string();
    }
    id.split(['-', '_'])
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Fetches and parses a model list. `url` is the full list URL. `headers` are the caller's header values with
/// `{session_id}` already resolved (see `resolve_headers_for_fetch`). Times out after `FETCH_TIMEOUT`.
pub async fn fetch_models(
    client: &reqwest::Client,
    url: &str,
    api_key: &str,
    headers: &[(String, String)],
    user_agent: &str,
) -> Result<Vec<FetchedModel>, ModelsError> {
    fetch_within(client, url, api_key, headers, user_agent, FETCH_TIMEOUT).await
}

async fn fetch_within(
    client: &reqwest::Client,
    url: &str,
    api_key: &str,
    headers: &[(String, String)],
    user_agent: &str,
    timeout: Duration,
) -> Result<Vec<FetchedModel>, ModelsError> {
    let cfg = ProviderConfig {
        chat_url: url.to_string(),
        api_key: api_key.to_string(),
        headers: headers.to_vec(),
        user_agent: user_agent.to_string(),
    };
    let header_map = llm::build_headers(&cfg).map_err(from_llm)?;
    let response = client.get(url).headers(header_map).timeout(timeout).send().await.map_err(transport)?;
    let status = response.status();
    let (bytes, too_big) = read_capped(response).await?;
    let body = String::from_utf8_lossy(&bytes);
    if !status.is_success() {
        // The same secrets the connection test removes: the key and every header value. The key is also replaced
        // exactly, so a short key that the redactor would skip is still removed.
        let mut known = vec![api_key.to_string()];
        known.extend(headers.iter().map(|(_, value)| value.clone()));
        let text = Redactor::new(known).redact(&llm::redact(&body, api_key));
        return Err(ModelsError::Http { status: status.as_u16(), message: error_message(&text) });
    }
    if too_big {
        return Err(ModelsError::TooLarge);
    }
    parse_model_list(&llm::redact(&body, api_key))
}

/// Reads the body up to `MAX_BODY_BYTES`. Returns what was read and whether the body was longer. Stops reading at
/// the cap, so the rest is never downloaded.
async fn read_capped(response: reqwest::Response) -> Result<(Vec<u8>, bool), ModelsError> {
    let mut body = Vec::new();
    let mut chunks = response.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(transport)?;
        let room = MAX_BODY_BYTES - body.len();
        if chunk.len() > room {
            body.extend_from_slice(&chunk[..room]);
            return Ok((body, true));
        }
        body.extend_from_slice(&chunk);
    }
    Ok((body, false))
}

/// The provider's message from an error body: `error.message` (or a string `error`) when the body is JSON, else the
/// body text. Already key-free (the caller redacts first). Cut to `MESSAGE_LIMIT` characters.
fn error_message(text: &str) -> String {
    let from_json = serde_json::from_str::<Value>(text.trim_start()).ok().and_then(|v| {
        let error = v.get("error")?;
        match error {
            Value::String(message) => Some(message.clone()),
            _ => error.get("message")?.as_str().map(str::to_string),
        }
    });
    let message = from_json.unwrap_or_else(|| text.trim().to_string());
    let message = if message.trim().is_empty() { "the response had an empty body".to_string() } else { message };
    if message.chars().count() <= MESSAGE_LIMIT {
        return message;
    }
    let mut cut: String = message.chars().take(MESSAGE_LIMIT).collect();
    cut.push_str("...");
    cut
}

fn transport(err: reqwest::Error) -> ModelsError {
    from_llm(llm::transport(err))
}

fn from_llm(err: LlmError) -> ModelsError {
    match err {
        LlmError::Transport(detail) => ModelsError::Transport(detail),
        other => ModelsError::Transport(other.to_string()),
    }
}

/// Folds a fetched list into a provider's models.
///
/// Custom models are kept exactly as they are. A fetched model that is still listed is updated: its provider context
/// becomes the newly reported value, its user override (`context_limit`) is kept, and its auto variants are replaced
/// by the newly reported ones while its manual variants are kept (a manual variant shadows an auto one with the same
/// id at resolution). The merged variants are capped at `MAX_VARIANTS_PER_MODEL`: manual variants are always kept and
/// auto variants beyond the cap are dropped in incoming order, so the merged model always validates. A fetched model
/// that is no longer listed is removed. Models that are new are appended with the fetched auto variants (also capped), sorted by id. An existing model keeps its place. A fetched id that a custom model already
/// uses is ignored, so ids stay unique. The settings default is not checked here; the caller decides what to do about
/// a removed default.
pub fn merge_fetched(provider: &mut Provider, fetched: Vec<FetchedModel>) {
    // First occurrence wins, and sorting gives the append order.
    let mut fresh: BTreeMap<String, FetchedModel> = BTreeMap::new();
    for model in fetched {
        if !fresh.contains_key(&model.id) {
            fresh.insert(model.id.clone(), model);
        }
    }
    let existing: HashSet<String> = provider.models.iter().map(|m| m.id.clone()).collect();

    let mut merged = Vec::with_capacity(provider.models.len() + fresh.len());
    for model in std::mem::take(&mut provider.models) {
        match model.source {
            ModelSource::Custom => merged.push(model),
            ModelSource::Fetched => {
                if let Some(new) = fresh.remove(&model.id) {
                    let mut kept: Vec<ModelVariant> =
                        model.variants.into_iter().filter(|v| v.source == VariantSource::Manual).collect();
                    let room = MAX_VARIANTS_PER_MODEL.saturating_sub(kept.len());
                    kept.extend(new.variants.into_iter().take(room));
                    merged.push(Model {
                        provider_context: new.context,
                        vision: new.vision,
                        variants: kept,
                        ..model
                    });
                }
            }
        }
    }
    for (id, new) in fresh {
        if !existing.contains(&id) {
            let mut variants = new.variants;
            variants.truncate(MAX_VARIANTS_PER_MODEL);
            merged.push(Model {
                id,
                context_limit: None,
                provider_context: new.context,
                vision: new.vision,
                source: ModelSource::Fetched,
                variants,
            });
        }
    }
    provider.models = merged;
}

/// The model-list URL: the explicit `models_url` if set, else the chat URL with its `/chat/completions` ending
/// replaced by `/models`. `None` when neither applies.
pub fn models_url_for(provider: &Provider) -> Option<String> {
    if let Some(url) = &provider.models_url {
        return Some(url.clone());
    }
    provider.chat_url.strip_suffix("/chat/completions").map(|base| format!("{base}/models"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(models: &[FetchedModel]) -> Vec<&str> {
        models.iter().map(|m| m.id.as_str()).collect()
    }

    fn fetched(id: &str, context: Option<u32>) -> FetchedModel {
        FetchedModel { id: id.into(), context, vision: None, variants: Vec::new() }
    }

    fn fetched_with(id: &str, variants: Vec<ModelVariant>) -> FetchedModel {
        FetchedModel { id: id.into(), context: None, vision: None, variants }
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

    fn provider_with(models: Vec<Model>) -> Provider {
        Provider {
            id: "p".into(),
            name: "P".into(),
            chat_url: "https://api.example.test/v1/chat/completions".into(),
            models_url: None,
            headers: Vec::new(),
            stream: true,
            max_tokens: None,
            models,
        }
    }

    // Shapes written by hand from the documented or observed responses (tests/fixtures holds the real captures).

    #[test]
    fn openrouter_shape_reads_context_length() {
        let body = r#"{"data":[
            {"id":"deepseek/deepseek-v4.1-flash","name":"DeepSeek: V4.1 Flash","context_length":1048576,
             "pricing":{"prompt":"0.000000051","completion":"0.000000567"},
             "top_provider":{"context_length":1048576,"max_completion_tokens":null,"is_moderated":false}},
            {"id":"only/top-provider","context_length":null,"top_provider":{"context_length":4096}}
        ],"total_count":2}"#;
        let models = parse_model_list(body).unwrap();
        assert_eq!(
            models,
            vec![fetched("deepseek/deepseek-v4.1-flash", Some(1_048_576)), fetched("only/top-provider", Some(4096))]
        );
    }

    #[test]
    fn opencode_go_shape_reads_limit_context() {
        let body = r#"{"object":"list","data":[
            {"id":"deepseek-v4.1-flash","object":"model","created":1,"owned_by":"opencode",
             "limit":{"context":1000000,"output":384000},"capabilities":{"tools":{"supported":true}}}
        ]}"#;
        assert_eq!(parse_model_list(body).unwrap(), vec![fetched("deepseek-v4.1-flash", Some(1_000_000))]);
    }

    #[test]
    fn plain_openai_shape_has_no_context() {
        let body = r#"{"object":"list","data":[{"id":"gpt-x","object":"model","created":1,"owned_by":"openai"}]}"#;
        assert_eq!(parse_model_list(body).unwrap(), vec![fetched("gpt-x", None)]);
    }

    #[test]
    fn other_root_shapes_and_entry_forms_are_read() {
        assert_eq!(parse_model_list(r#"["a","b"]"#).unwrap(), vec![fetched("a", None), fetched("b", None)]);
        assert_eq!(
            parse_model_list(r#"{"models":[{"name":"m1"},{"model":"m2"},{"id":"m3","context_window":8192}]}"#).unwrap(),
            vec![fetched("m1", None), fetched("m2", None), fetched("m3", Some(8192))]
        );
        assert_eq!(parse_model_list(r#"{"data":[]}"#).unwrap(), Vec::new());
    }

    #[test]
    fn leading_whitespace_is_tolerated() {
        assert_eq!(parse_model_list("\n\n  {\"data\":[{\"id\":\"a\"}]}").unwrap(), vec![fetched("a", None)]);
    }

    #[test]
    fn bodies_that_are_not_model_lists_are_refused() {
        for body in ["not json", "\"just a string\"", "42", r#"{"foo":1}"#, r#"{"data":"nope"}"#] {
            assert!(matches!(parse_model_list(body), Err(ModelsError::Parse(_))), "{body}");
        }
    }

    #[test]
    fn context_takes_the_first_valid_field_only() {
        let cases = [
            (r#"{"id":"a","context_length":0,"context_window":8192}"#, Some(8192)),
            (r#"{"id":"a","context_length":4096.0}"#, None),
            (r#"{"id":"a","context_length":"4096"}"#, None),
            (r#"{"id":"a","context_length":-1}"#, None),
            (r#"{"id":"a","context_length":200000000,"context":2048}"#, Some(2048)),
            (r#"{"id":"a","context_length":100000000}"#, Some(100_000_000)),
            (r#"{"id":"a","limit":{"context":5000},"max_context_length":6000}"#, Some(5000)),
            (r#"{"id":"a","top_provider":{"context_length":7000},"context":9000}"#, Some(7000)),
            (r#"{"id":"a","max_context_length":6000,"context":9000}"#, Some(6000)),
            (r#"{"id":"a","context":9000}"#, Some(9000)),
        ];
        for (entry, expected) in cases {
            let models = parse_model_list(&format!(r#"{{"data":[{entry}]}}"#)).unwrap();
            assert_eq!(models[0].context, expected, "{entry}");
        }
    }

    #[test]
    fn ids_are_chosen_and_filtered_like_settings_ids() {
        let long = "x".repeat(MAX_MODEL_ID_CHARS + 1);
        let body = format!(
            r#"{{"data":[
                {{"id":""}},
                {{"id":"   "}},
                {{"name":"fallback-name"}},
                {{"id":"bad\tid"}},
                {{"id":"{long}"}},
                {{"id":"{ok}"}},
                {{"id":"dup","context_length":1000}},
                {{"id":"dup","context_length":2000}},
                42,
                null,
                "plain"
            ]}}"#,
            ok = "y".repeat(MAX_MODEL_ID_CHARS),
        );
        let models = parse_model_list(&body).unwrap();
        let max_len_id = "y".repeat(MAX_MODEL_ID_CHARS);
        assert_eq!(ids(&models), vec!["fallback-name", max_len_id.as_str(), "dup", "plain"]);
        assert_eq!(models[2].context, Some(1000), "the first of a repeated id is kept");
    }

    #[test]
    fn at_most_two_thousand_models_are_kept() {
        let entries: Vec<String> = (0..2500).map(|i| format!(r#"{{"id":"m{i}"}}"#)).collect();
        let body = format!(r#"{{"data":[{}]}}"#, entries.join(","));
        let models = parse_model_list(&body).unwrap();
        assert_eq!(models.len(), 2000);
        assert_eq!(models[1999].id, "m1999");
    }

    #[test]
    fn error_messages_are_taken_from_the_json_and_cut() {
        assert_eq!(error_message(r#"{"error":{"message":"Invalid key","code":401}}"#), "Invalid key");
        assert_eq!(error_message(r#"{"error":"plain"}"#), "plain");
        assert_eq!(error_message("  Gateway down  "), "Gateway down");
        assert_eq!(error_message("   "), "the response had an empty body");
        let long = error_message(&"e".repeat(1000));
        assert_eq!(long.chars().count(), MESSAGE_LIMIT + 3);
    }

    #[test]
    fn merge_updates_keeps_overrides_removes_and_appends_sorted() {
        let mut provider = provider_with(vec![
            Model {
                id: "mine".into(),
                context_limit: Some(5000),
                provider_context: None,
                vision: None,
                source: ModelSource::Custom,
                variants: Vec::new(),
            },
            Model {
                id: "kept".into(),
                context_limit: Some(9000),
                provider_context: Some(1),
                vision: None,
                source: ModelSource::Fetched,
                variants: Vec::new(),
            },
            Model {
                id: "gone".into(),
                context_limit: None,
                provider_context: Some(3),
                vision: None,
                source: ModelSource::Fetched,
                variants: Vec::new(),
            },
        ]);
        merge_fetched(
            &mut provider,
            vec![
                fetched("kept", Some(2000)),
                fetched("zeta", None),
                fetched("alpha", Some(3000)),
                fetched("mine", Some(7777)),
                fetched("alpha", Some(4000)),
            ],
        );

        let order: Vec<&str> = provider.models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(order, vec!["mine", "kept", "alpha", "zeta"]);

        let mine = &provider.models[0];
        assert_eq!(mine.source, ModelSource::Custom);
        assert_eq!((mine.context_limit, mine.provider_context), (Some(5000), None), "custom models are untouched");

        let kept = &provider.models[1];
        assert_eq!(kept.source, ModelSource::Fetched);
        assert_eq!(kept.context_limit, Some(9000), "the user's override survives a refresh");
        assert_eq!(kept.provider_context, Some(2000), "the provider's value is updated");

        assert_eq!(provider.models[2].provider_context, Some(3000), "the first of a repeated id is used");
        assert_eq!(provider.models[2].context_limit, None);
        assert_eq!(provider.models[3].provider_context, None);
    }

    /// The modality of each entry: from `architecture.input_modalities`, None without a usable list.
    #[test]
    fn vision_is_read_from_the_input_modalities() {
        let body = r#"{"data":[
            {"id":"a/vision","architecture":{"input_modalities":["text","image"]}},
            {"id":"b/text","architecture":{"input_modalities":["text"]}},
            {"id":"c/none"},
            {"id":"d/not-a-list","architecture":{"input_modalities":"image"}},
            {"id":"e/empty","architecture":{"input_modalities":[]}}
        ]}"#;
        let models = parse_model_list(body).unwrap();
        let vision: Vec<(&str, Option<bool>)> = models.iter().map(|m| (m.id.as_str(), m.vision)).collect();
        assert_eq!(
            vision,
            vec![
                ("a/vision", Some(true)),
                ("b/text", Some(false)),
                ("c/none", None),
                ("d/not-a-list", None),
                ("e/empty", Some(false)),
            ]
        );
    }

    /// A refresh sets the modality of fetched models (kept and new), and never touches a custom model's.
    #[test]
    fn merge_carries_the_modality_and_leaves_custom_models_alone() {
        let mut provider = provider_with(vec![
            Model { id: "mine".into(), context_limit: None, provider_context: None, vision: None, source: ModelSource::Custom, variants: Vec::new() },
            Model { id: "kept".into(), context_limit: None, provider_context: None, vision: Some(true), source: ModelSource::Fetched, variants: Vec::new() },
        ]);
        let fresh =
            |id: &str, vision| FetchedModel { id: id.into(), context: None, vision, variants: Vec::new() };
        merge_fetched(&mut provider, vec![fresh("kept", Some(false)), fresh("new", Some(true)), fresh("mine", Some(true))]);

        let vision: Vec<(&str, Option<bool>)> = provider.models.iter().map(|m| (m.id.as_str(), m.vision)).collect();
        assert_eq!(vision, vec![("mine", None), ("kept", Some(false)), ("new", Some(true))]);
    }

    #[test]
    fn merge_with_an_empty_list_removes_fetched_models_only() {
        let mut provider = provider_with(vec![
            Model { id: "c".into(), context_limit: None, provider_context: None, vision: None, source: ModelSource::Custom, variants: Vec::new() },
            Model { id: "f".into(), context_limit: None, provider_context: None, vision: None, source: ModelSource::Fetched, variants: Vec::new() },
        ]);
        merge_fetched(&mut provider, Vec::new());
        assert_eq!(provider.models.len(), 1);
        assert_eq!(provider.models[0].id, "c");
    }

    fn variant_ids(model: &FetchedModel) -> Vec<&str> {
        model.variants.iter().map(|v| v.id.as_str()).collect()
    }

    #[test]
    fn openrouter_efforts_become_reasoning_variants() {
        let body = r#"{"data":[{
            "id": "m",
            "reasoning": {"supported_efforts": ["low", "high", "xhigh", "none", "Minimal"]}
        }]}"#;
        let models = parse_model_list(body).unwrap();
        assert_eq!(variant_ids(&models[0]), vec!["low", "high", "xhigh", "none", "minimal"]);
        for variant in &models[0].variants {
            assert_eq!(variant.source, VariantSource::Auto, "{:?}", variant.id);
            assert_eq!(variant.body["reasoning"]["effort"], serde_json::Value::String(variant.id.clone()));
        }
        let labels: Vec<&str> = models[0].variants.iter().map(|v| v.label.as_str()).collect();
        assert_eq!(labels, vec!["Low", "High", "X High", "None", "Minimal"]);
    }

    #[test]
    fn opencode_efforts_become_reasoning_effort_variants() {
        let body = r#"{"data":[{
            "id": "m",
            "capabilities": {"reasoning": {"effort": ["low", "medium"]}}
        }]}"#;
        let models = parse_model_list(body).unwrap();
        assert_eq!(variant_ids(&models[0]), vec!["low", "medium"]);
        assert_eq!(models[0].variants[0].body, serde_json::from_str(r#"{"reasoning_effort":"low"}"#).unwrap());
        assert_eq!(models[0].variants[1].label, "Medium");
        assert!(models[0].variants.iter().all(|v| v.source == VariantSource::Auto));
    }

    #[test]
    fn openrouter_shape_wins_over_the_opencode_shape() {
        let body = r#"{"data":[{
            "id": "m",
            "reasoning": {"supported_efforts": ["high"]},
            "capabilities": {"reasoning": {"effort": ["low"]}}
        }]}"#;
        let models = parse_model_list(body).unwrap();
        assert_eq!(variant_ids(&models[0]), vec!["high"]);
        assert_eq!(models[0].variants[0].body["reasoning"]["effort"], serde_json::Value::String("high".into()));
    }

    #[test]
    fn non_array_effort_fields_fall_through_to_the_opencode_shape() {
        // `reasoning.supported_efforts` is not an array here, so the OpenCode path applies.
        let body = r#"{"data":[{
            "id": "m",
            "reasoning": {"supported_efforts": "high"},
            "capabilities": {"reasoning": {"effort": ["low"]}}
        }]}"#;
        assert_eq!(variant_ids(&parse_model_list(body).unwrap()[0]), vec!["low"]);

        let body = r#"{"data":[{
            "id": "m",
            "reasoning": {"supported_efforts": "high"}
        }]}"#;
        assert!(parse_model_list(body).unwrap()[0].variants.is_empty());
    }

    #[test]
    fn skipped_malformed_and_repeated_efforts_are_dropped() {
        let long = "e".repeat(33);
        let body = serde_json::json!({"data": [{
            "id": "m",
            "reasoning": {"supported_efforts": [
                "toggle", "budget", "default_effort", "mandatory", "TOGGLE",
                "High", "HIGH", "high",
                "", "bad id!", &long, "0",
                42, true, null, {"effort": "low"}, ["low"],
                "none",
            ]},
        }]});
        let models = parse_model_list(&body.to_string()).unwrap();
        assert_eq!(variant_ids(&models[0]), vec!["high", "0", "none"]);
    }

    #[test]
    fn at_most_sixteen_effort_variants_are_kept() {
        let efforts: Vec<serde_json::Value> =
            (0..20).map(|i| serde_json::Value::String(format!("effort-{i:02}"))).collect();
        let body = serde_json::json!({"data": [{
            "id": "m",
            "capabilities": {"reasoning": {"effort": efforts}},
        }]});
        let models = parse_model_list(&body.to_string()).unwrap();
        assert_eq!(models[0].variants.len(), 16);
        assert_eq!(models[0].variants[15].id, "effort-15");
        assert_eq!(models[0].variants[15].label, "Effort 15");
    }

    #[test]
    fn entries_without_effort_metadata_have_no_variants() {
        let body = r#"{"data":[{"id":"plain"},{"id":"other","reasoning":{}}]}"#;
        let models = parse_model_list(body).unwrap();
        assert!(models.iter().all(|m| m.variants.is_empty()));
    }

    #[test]
    fn merge_replaces_auto_variants_and_keeps_manual_ones() {
        let mut provider = provider_with(vec![
            Model {
                id: "mine".into(),
                context_limit: None,
                provider_context: None,
                vision: None,
                source: ModelSource::Custom,
                variants: vec![manual_variant("custom")],
            },
            Model {
                id: "kept".into(),
                context_limit: None,
                provider_context: None,
                vision: None,
                source: ModelSource::Fetched,
                variants: vec![manual_variant("mine"), auto_variant("old")],
            },
        ]);
        merge_fetched(
            &mut provider,
            vec![
                fetched_with("kept", vec![auto_variant("new")]),
                fetched_with("fresh", vec![auto_variant("auto")]),
                fetched_with("mine", vec![auto_variant("ignored")]),
            ],
        );

        let mine = provider.model("mine").unwrap();
        assert_eq!(mine.source, ModelSource::Custom);
        assert_eq!(mine.variants, vec![manual_variant("custom")], "custom models are untouched");

        let kept = provider.model("kept").unwrap();
        let summary: Vec<(&str, VariantSource)> =
            kept.variants.iter().map(|v| (v.id.as_str(), v.source)).collect();
        assert_eq!(
            summary,
            vec![("mine", VariantSource::Manual), ("new", VariantSource::Auto)],
            "manual variants are kept, auto ones replaced"
        );

        let fresh = provider.model("fresh").unwrap();
        assert_eq!(fresh.variants, vec![auto_variant("auto")], "new models get the fetched variants");
    }

    #[test]
    fn a_manual_variant_survives_when_the_auto_list_disappears() {
        let mut provider = provider_with(vec![Model {
            id: "kept".into(),
            context_limit: None,
            provider_context: None,
            vision: None,
            source: ModelSource::Fetched,
            variants: vec![manual_variant("mine"), auto_variant("old")],
        }]);
        merge_fetched(&mut provider, vec![fetched("kept", None)]);
        assert_eq!(provider.model("kept").unwrap().variants, vec![manual_variant("mine")]);
    }

    #[test]
    fn merge_keeps_all_manual_and_truncates_auto_to_the_cap() {
        let manuals = vec![manual_variant("m0"), manual_variant("m1"), manual_variant("m2")];
        let mut provider = provider_with(vec![Model {
            id: "kept".into(),
            context_limit: None,
            provider_context: None,
            vision: None,
            source: ModelSource::Fetched,
            variants: manuals.clone(),
        }]);
        let incoming: Vec<ModelVariant> = (0..20).map(|i| auto_variant(&format!("v{i:02}"))).collect();
        merge_fetched(&mut provider, vec![fetched_with("kept", incoming)]);

        let kept = provider.model("kept").unwrap();
        assert_eq!(kept.variants.len(), MAX_VARIANTS_PER_MODEL);
        assert_eq!(&kept.variants[..manuals.len()], &manuals[..], "all manual variants are kept");
        let auto_ids: Vec<&str> = kept.variants[manuals.len()..].iter().map(|v| v.id.as_str()).collect();
        let expected: Vec<String> = (0..MAX_VARIANTS_PER_MODEL - manuals.len()).map(|i| format!("v{i:02}")).collect();
        assert_eq!(
            auto_ids,
            expected.iter().map(String::as_str).collect::<Vec<_>>(),
            "auto variants are kept in incoming order up to the cap"
        );
        assert!(kept.variants[manuals.len()..].iter().all(|v| v.source == VariantSource::Auto));
    }

    #[test]
    fn merged_variant_totals_never_exceed_the_cap() {
        let mut provider = provider_with(vec![Model {
            id: "kept".into(),
            context_limit: None,
            provider_context: None,
            vision: None,
            source: ModelSource::Fetched,
            variants: vec![manual_variant("m0")],
        }]);
        let many: Vec<ModelVariant> = (0..20).map(|i| auto_variant(&format!("v{i:02}"))).collect();
        let fresh_many: Vec<ModelVariant> = (0..20).map(|i| auto_variant(&format!("n{i:02}"))).collect();
        merge_fetched(
            &mut provider,
            vec![fetched_with("kept", many), fetched_with("fresh", fresh_many)],
        );

        for model in &provider.models {
            assert!(
                model.variants.len() <= MAX_VARIANTS_PER_MODEL,
                "model {:?} has {} variants",
                model.id,
                model.variants.len()
            );
        }
        assert_eq!(provider.model("kept").unwrap().variants.len(), MAX_VARIANTS_PER_MODEL);
        assert_eq!(provider.model("fresh").unwrap().variants.len(), MAX_VARIANTS_PER_MODEL);
        let fresh_ids: Vec<&str> =
            provider.model("fresh").unwrap().variants.iter().map(|v| v.id.as_str()).collect();
        let expected: Vec<String> = (0..MAX_VARIANTS_PER_MODEL).map(|i| format!("n{i:02}")).collect();
        assert_eq!(fresh_ids, expected.iter().map(String::as_str).collect::<Vec<_>>());

        let settings = crate::core::settings::Settings {
            providers: vec![provider],
            ..Default::default()
        };
        assert_eq!(settings.validate(), Ok(()), "the capped merge still validates");
    }

    #[test]
    fn default_variant_survives_truncation_when_kept_and_is_gone_when_truncated() {
        let manuals = vec![manual_variant("m0"), manual_variant("m1")];
        let mut provider = provider_with(vec![Model {
            id: "kept".into(),
            context_limit: None,
            provider_context: None,
            vision: None,
            source: ModelSource::Fetched,
            variants: manuals.clone(),
        }]);
        // Two manuals leave room for 14 autos: v00..v13 survive, v14.. are dropped.
        let incoming: Vec<ModelVariant> = (0..20).map(|i| auto_variant(&format!("v{i:02}"))).collect();
        merge_fetched(&mut provider, vec![fetched_with("kept", incoming)]);
        let kept = provider.model("kept").unwrap();

        assert!(kept.variant("m0").is_some(), "a manual default always survives");
        assert!(kept.variant("v05").is_some(), "a default auto variant within the cap survives");
        assert_eq!(kept.variant("v05").unwrap().source, VariantSource::Auto);
        assert!(kept.variant("v19").is_none(), "a default auto variant beyond the cap is gone");

        // What `apply_fetched` checks before clearing: the surviving default still resolves, so it is kept;
        // the truncated one no longer does, so the existing clearing logic drops it.
        let survives = kept.variant("v05").is_none();
        let truncated = kept.variant("v19").is_none();
        assert!(!survives, "the surviving default would be kept by apply_fetched");
        assert!(truncated, "the truncated default would be cleared by apply_fetched");
    }

    #[test]
    fn models_url_is_explicit_or_derived_from_the_chat_url() {
        let mut provider = provider_with(Vec::new());
        assert_eq!(
            models_url_for(&provider).as_deref(),
            Some("https://api.example.test/v1/models"),
            "derived from /chat/completions"
        );
        provider.models_url = Some("https://other.example.test/list".into());
        assert_eq!(models_url_for(&provider).as_deref(), Some("https://other.example.test/list"));
        provider.models_url = None;
        provider.chat_url = "https://api.example.test/v1/responses".into();
        assert_eq!(models_url_for(&provider), None);
    }

    /// Accepts one connection, sends the head, then holds the socket open without finishing the body.
    fn stalling_server() -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let _ = sock.read(&mut buf);
            let _ = sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{\"data\":");
            let _ = sock.flush();
            std::thread::sleep(Duration::from_secs(3));
        });
        format!("http://{addr}/v1/models")
    }

    #[tokio::test]
    async fn a_stalled_body_hits_the_total_timeout() {
        let url = stalling_server();
        let client = llm::http_client().unwrap();
        let err = fetch_within(&client, &url, "test-key", &[], "watt-terminal-test/0", Duration::from_millis(300))
            .await
            .unwrap_err();
        assert_eq!(err, ModelsError::Transport("the request timed out".to_string()));
    }

    /// Answers one request with `status` and `body`, then closes the connection.
    fn one_shot(status: &'static str, body: String) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let _ = sock.read(&mut buf);
            let head = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = sock.write_all(head.as_bytes());
            let _ = sock.write_all(body.as_bytes());
        });
        format!("http://{addr}/v1/models")
    }

    #[tokio::test]
    async fn an_error_body_hides_the_key_and_every_header_value() {
        const KEY: &str = "DUMMY-key-0123456789abcdef";
        const HEADER: &str = "DUMMY-header-fedcba9876543210";
        let body = format!(r#"{{"error":{{"message":"bad key {KEY} and header {HEADER}"}}}}"#);
        let url = one_shot("403 Forbidden", body);
        let client = llm::http_client().unwrap();
        let headers = [("X-Key".to_string(), HEADER.to_string())];
        let err = fetch_within(&client, &url, KEY, &headers, "watt-terminal-test/0", FETCH_TIMEOUT).await.unwrap_err();
        let text = err.to_string();
        assert!(matches!(err, ModelsError::Http { status: 403, .. }), "{err:?}");
        assert!(!text.contains(KEY) && !text.contains(HEADER), "{text}");
        assert!(text.contains("bad key [redacted] and header [REDACTED]"), "{text}");
    }
}
