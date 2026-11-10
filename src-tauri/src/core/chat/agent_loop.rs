//! The chat agent's turn loop: model calls, tool calls, and the history that carries them.
//!
//! `LlmAgent` implements `Agent`. A turn pushes the user message, then makes model calls until the model answers
//! without tool calls. Each reply's tool calls run one by one through `AgentCtx`, so approvals, events and the shell
//! gate are the same as for the mock. Every result is projected and redacted (core/chat/project) before it enters the
//! history. A denial or a cancel stops the turn: the calls not yet run are closed in the history, and no further
//! model call is made. Every exit path keeps the pairing that `History` checks: no call is left without a result.
//!
//! Dev configuration (replaced by a config file later): `AGENT_TERM_LLM_URL` (full chat-completions URL),
//! `AGENT_TERM_LLM_KEY` and `AGENT_TERM_LLM_MODEL` must all be set, or the mock agent is used. Optional:
//! `AGENT_TERM_LLM_HEADERS` (`Name: value` lines or `;`-separated, where `{session_id}` is the conversation id),
//! `AGENT_TERM_LLM_STREAM=0` for one non-streamed reply per call, and `AGENT_TERM_LLM_MAX_TOKENS`.
//! Nothing here logs the key or a header value.

use std::cell::Cell;
use std::fmt;
use std::fs::File;
use std::future::Future;
use std::io::{self, Read};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use reqwest::header::{HeaderName, HeaderValue};
use serde_json::Value;

use crate::core::agent::{Agent, AgentCtx, AgentEvent, DENIED};
use crate::core::chat::history::History;
use crate::core::chat::project::project;
use crate::core::chat::prompt;
use crate::core::chat::redact::Redactor;
use crate::core::llm::{self, AssistantReply, ChatRequest, Delta, LlmError, ProviderConfig, ToolCall, TurnCancel};

/// Model calls allowed in one turn. The tool calls of the last one are not run.
pub const MAX_MODEL_CALLS: usize = 50;
/// `run_command` calls allowed in one turn. A call past the cap gets an error result and does not run.
pub const MAX_COMMANDS_PER_TURN: usize = 30;

const SESSION_PLACEHOLDER: &str = "{session_id}";
pub const USER_AGENT: &str = concat!("agent-term/", env!("CARGO_PKG_VERSION"));
const ENV_URL: &str = "AGENT_TERM_LLM_URL";
const ENV_KEY: &str = "AGENT_TERM_LLM_KEY";
const ENV_MODEL: &str = "AGENT_TERM_LLM_MODEL";
const ENV_HEADERS: &str = "AGENT_TERM_LLM_HEADERS";
const ENV_STREAM: &str = "AGENT_TERM_LLM_STREAM";
const ENV_MAX_TOKENS: &str = "AGENT_TERM_LLM_MAX_TOKENS";

/// Tool names the model may call that the loop routes by name. Anything else is an unknown tool. Must match `READ_TOOLS`
/// in core/agent.rs.
const READ_NAMES: [&str; 5] = ["list_commands", "get_command", "get_screen", "get_cwd", "wait"];
/// Longest tool name echoed back in an error, so a model cannot make the history carry a huge name.
const NAME_LIMIT: usize = 80;

const LIMIT_REASON: &str = "not run: the turn stopped after the model call limit";
const DENIED_NOTICE: &str = "Stopped: the command was not approved. Tell me what you want to do next.";

/// The provider settings read from the environment. Holds the key, so `Debug` hides it.
#[derive(Clone)]
pub struct LlmSettings {
    pub chat_url: String,
    pub api_key: String,
    pub model: String,
    /// Header templates as written. `{session_id}` is replaced per conversation by `resolved_headers`.
    pub headers: Vec<(String, String)>,
    /// True streams the reply (text appears as it arrives); false waits for the whole reply.
    pub stream: bool,
    pub max_tokens: Option<u32>,
    /// The context limit that applies to this model, in tokens (core/settings). Not used by the loop yet.
    pub context_limit: Option<u32>,
}

impl LlmSettings {
    /// The header values with `{session_id}` replaced by the conversation id.
    pub fn resolved_headers(&self, conversation_id: &str) -> Vec<(String, String)> {
        self.headers
            .iter()
            .map(|(name, value)| (name.clone(), value.replace(SESSION_PLACEHOLDER, conversation_id)))
            .collect()
    }
}

impl fmt::Debug for LlmSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let header_names: Vec<&str> = self.headers.iter().map(|(name, _)| name.as_str()).collect();
        f.debug_struct("LlmSettings")
            .field("chat_url", &self.chat_url)
            .field("api_key", &"<redacted>")
            .field("model", &self.model)
            .field("header_names", &header_names)
            .field("stream", &self.stream)
            .field("max_tokens", &self.max_tokens)
            .field("context_limit", &self.context_limit)
            .finish()
    }
}

/// Reads the settings from `lookup` (the environment in production). `Ok(None)` when URL, key or model is missing:
/// the mock agent is used. `Err` is a bad value, with a message that never includes a secret.
pub fn settings_from_env(lookup: impl Fn(&str) -> Option<String>) -> Result<Option<LlmSettings>, String> {
    let get = |name: &str| lookup(name).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    let (Some(chat_url), Some(api_key), Some(model)) = (get(ENV_URL), get(ENV_KEY), get(ENV_MODEL)) else {
        return Ok(None);
    };
    if !(chat_url.starts_with("https://") || chat_url.starts_with("http://")) {
        return Err(format!("{ENV_URL} must start with https:// or http://"));
    }
    let headers = parse_headers(&get(ENV_HEADERS).unwrap_or_default())?;
    let stream = match get(ENV_STREAM).as_deref() {
        None | Some("1") => true,
        Some("0") => false,
        Some(_) => return Err(format!("{ENV_STREAM} must be 0 or 1")),
    };
    let max_tokens = match get(ENV_MAX_TOKENS) {
        None => None,
        Some(v) => {
            let n = v.parse::<u32>().ok().filter(|n| *n > 0);
            Some(n.ok_or_else(|| format!("{ENV_MAX_TOKENS} must be a positive whole number"))?)
        }
    };
    Ok(Some(LlmSettings { chat_url, api_key, model, headers, stream, max_tokens, context_limit: None }))
}

/// `Name: value` entries, one per line or separated by `;`. Each name and value is checked now, with the placeholder
/// filled by a stand-in, so a bad header fails at startup rather than on the first request.
fn parse_headers(text: &str) -> Result<Vec<(String, String)>, String> {
    let entries = text.split(['\n', ';']).map(str::trim).filter(|e| !e.is_empty());
    let mut out = Vec::new();
    for (index, entry) in entries.enumerate() {
        let number = index + 1;
        let (name, value) = entry
            .split_once(':')
            .map(|(name, value)| (name.trim(), value.trim()))
            .filter(|(name, _)| !name.is_empty())
            .ok_or_else(|| format!("{ENV_HEADERS} entry {number} is not `Name: value`"))?;
        HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| format!("{ENV_HEADERS} entry {number} has an invalid header name"))?;
        HeaderValue::from_str(&value.replace(SESSION_PLACEHOLDER, "ses_0"))
            .map_err(|_| format!("{ENV_HEADERS} entry {number} has an invalid header value"))?;
        out.push((name.to_string(), value.to_string()));
    }
    Ok(out)
}

/// Reads the settings once per process from the environment. A bad value is logged (without the value) and the mock
/// agent is used. `run()` calls this at startup so the warning shows at launch.
pub fn settings() -> Option<&'static LlmSettings> {
    static SETTINGS: OnceLock<Option<LlmSettings>> = OnceLock::new();
    SETTINGS.get_or_init(|| settings_or_warn(|name| std::env::var(name).ok())).as_ref()
}

/// An owned copy of `settings()`, for callers that keep the settings past the turn (the agent takes ownership).
pub fn settings_cloned() -> Option<LlmSettings> {
    settings().cloned()
}

fn settings_or_warn(lookup: impl Fn(&str) -> Option<String>) -> Option<LlmSettings> {
    match settings_from_env(lookup) {
        Ok(settings) => settings,
        Err(problem) => {
            eprintln!("agent-term: {problem}. The mock agent is used instead.");
            None
        }
    }
}

static CLIENT: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();

/// The one HTTP client shared by every conversation.
pub fn shared_client() -> Result<&'static reqwest::Client, String> {
    CLIENT.get_or_init(|| llm::http_client().map_err(|e| e.to_string())).as_ref().map_err(|e| e.clone())
}

/// A new conversation id: "ses_" and 32 lowercase hex digits from the OS random source.
pub fn new_conversation_id() -> io::Result<String> {
    let mut bytes = [0u8; 16];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(format!("ses_{}", bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()))
}

/// One session's conversation: the id sent to the provider and the history. `reset` starts a new one.
pub struct Conversation {
    id: Mutex<String>,
    history: Arc<Mutex<History>>,
}

impl Conversation {
    pub fn new() -> io::Result<Self> {
        Ok(Conversation { id: Mutex::new(new_conversation_id()?), history: Arc::new(Mutex::new(History::new())) })
    }

    pub fn id(&self) -> String {
        lock_string(&self.id).clone()
    }

    pub fn history(&self) -> Arc<Mutex<History>> {
        self.history.clone()
    }

    /// Clears the history and rotates the id. The caller makes sure no turn is running.
    pub fn reset(&self) -> io::Result<()> {
        let id = new_conversation_id()?;
        lock(&self.history).clear();
        *lock_string(&self.id) = id;
        Ok(())
    }

    /// Makes a saved conversation the live one: its id and its history replace the current ones. The history object
    /// stays the same, so holders of `history()` see the new contents. The caller makes sure no turn is running.
    pub fn restore(&self, id: String, history: History) {
        *lock_string(&self.id) = id;
        *lock(&self.history) = history;
    }
}

/// The agent for one turn of a conversation: the provider config with this conversation's headers, the redactor, and
/// the shared history and cancel token.
pub struct LlmAgent {
    client: reqwest::Client,
    cfg: ProviderConfig,
    model: String,
    max_tokens: Option<u32>,
    stream: bool,
    redactor: Redactor,
    history: Arc<Mutex<History>>,
    cancel: Arc<TurnCancel>,
    system: String,
    tools: Vec<Value>,
}

impl LlmAgent {
    pub fn new(
        client: &reqwest::Client,
        settings: LlmSettings,
        conversation_id: &str,
        history: Arc<Mutex<History>>,
        cancel: Arc<TurnCancel>,
    ) -> Self {
        let headers = settings.resolved_headers(conversation_id);
        // The key and the header values are the secrets the output must never show.
        let mut known = vec![settings.api_key.clone()];
        known.extend(headers.iter().map(|(_, value)| value.clone()));
        LlmAgent {
            client: client.clone(),
            cfg: ProviderConfig {
                chat_url: settings.chat_url,
                api_key: settings.api_key,
                headers,
                user_agent: USER_AGENT.to_string(),
            },
            model: settings.model,
            max_tokens: settings.max_tokens,
            stream: settings.stream,
            redactor: Redactor::new(known),
            history,
            cancel,
            system: prompt::system_prompt(),
            tools: prompt::tool_definitions(),
        }
    }

    /// The redactor for this turn's secrets. The persistence layer uses it to scrub what it writes to disk.
    pub fn redactor(&self) -> &Redactor {
        &self.redactor
    }

    /// One model call. Streams text as it arrives, or waits for the whole reply. Returns None when the call was
    /// cancelled or failed; a failure has already been shown to the user.
    fn ask_model(&self, ctx: &AgentCtx<'_>) -> Option<AssistantReply> {
        let messages = lock(&self.history).messages(&self.system);
        let req = ChatRequest {
            model: self.model.clone(),
            messages,
            tools: Some(self.tools.clone()),
            max_tokens: self.max_tokens,
        };
        let thinking = Cell::new(true);
        ctx.emit(AgentEvent::Thinking { active: true });
        let result = if self.stream {
            let mut on_delta = |delta: Delta| {
                // Reasoning deltas are not shown yet, so they do not end the "thinking" state.
                if let Delta::Text(text) = delta
                    && !text.is_empty()
                {
                    stop_thinking(ctx, &thinking);
                    ctx.emit(AgentEvent::TextDelta { text });
                }
            };
            block(llm::stream(&self.client, &self.cfg, &req, &mut on_delta, &self.cancel))
        } else {
            block(llm::complete(&self.client, &self.cfg, &req, &self.cancel))
        };
        stop_thinking(ctx, &thinking);
        match result {
            Ok(reply) => {
                if !self.stream && !reply.text.is_empty() {
                    ctx.say(self.redactor.redact(&reply.text));
                }
                Some(reply)
            }
            Err(LlmError::Cancelled) => None,
            Err(e) => {
                ctx.say(self.redactor.redact(&error_text(&e)));
                None
            }
        }
    }

    /// Runs the tool calls of one reply in order and records each result. Returns why the turn must stop, if it must.
    /// The calls after a stop are not run; the caller closes them in the history.
    fn run_calls(&self, calls: &[ToolCall], ctx: &AgentCtx<'_>, commands: &mut usize) -> Option<Stop> {
        for call in calls {
            if self.cancel.is_cancelled() {
                return Some(Stop::Cancelled);
            }
            let (content, denied) = self.run_call(call, ctx, commands);
            lock(&self.history).push_tool_result(&call.id, &content);
            if self.cancel.is_cancelled() {
                return Some(Stop::Cancelled);
            }
            if denied {
                return Some(Stop::Denied);
            }
        }
        None
    }

    /// Runs one call. Returns the model-facing text of its result and whether the user denied it.
    fn run_call(&self, call: &ToolCall, ctx: &AgentCtx<'_>, commands: &mut usize) -> (String, bool) {
        let name = call.name.as_str();
        let failed = |message: &str| (project(name, false, &Value::String(message.to_string()), &self.redactor), false);
        let Ok(args) = serde_json::from_str::<Value>(&call.arguments) else {
            return failed("arguments were not valid JSON");
        };
        let result = match name {
            "run_command" => {
                if *commands >= MAX_COMMANDS_PER_TURN {
                    return failed("per-turn command limit reached; ask the user to continue");
                }
                *commands += 1;
                ctx.call_tool("run_command", args)
            }
            _ if READ_NAMES.contains(&name) => ctx.read_tool(name, args),
            "send_keys" => return failed("send_keys is not available in this version"),
            _ => {
                let shown: String = name.chars().take(NAME_LIMIT).collect();
                return failed(&format!("unknown tool `{shown}`"));
            }
        };
        let denied = matches!(&result, Err(e) if e.starts_with(DENIED));
        let content = match result {
            Ok(value) => project(name, true, &value, &self.redactor),
            Err(e) => project(name, false, &Value::String(e), &self.redactor),
        };
        (content, denied)
    }
}

impl Agent for LlmAgent {
    fn respond(&mut self, user_text: &str, ctx: &AgentCtx<'_>) {
        lock(&self.history).push_user(user_text);
        let mut commands = 0;
        for call_no in 1..=MAX_MODEL_CALLS {
            if self.cancel.is_cancelled() {
                return;
            }
            let Some(reply) = self.ask_model(ctx) else {
                return;
            };
            lock(&self.history).push_assistant(reply.raw);
            if reply.tool_calls.is_empty() {
                return;
            }
            if call_no == MAX_MODEL_CALLS {
                // The calls of the last allowed reply are not run, so nothing happens after the limit.
                lock(&self.history).close_pending(LIMIT_REASON);
                ctx.say(format!("I stopped after {MAX_MODEL_CALLS} model calls in one turn."));
                return;
            }
            if let Some(stop) = self.run_calls(&reply.tool_calls, ctx, &mut commands) {
                lock(&self.history).close_pending(stop.reason());
                if stop == Stop::Denied {
                    ctx.say(DENIED_NOTICE);
                }
                return;
            }
        }
    }
}

/// Why a turn stopped before the model answered.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stop {
    Denied,
    Cancelled,
}

impl Stop {
    fn reason(self) -> &'static str {
        match self {
            Stop::Denied => "denied by the user",
            Stop::Cancelled => "cancelled",
        }
    }
}

/// Runs an async adapter call on Tauri's runtime, so the shared client's connections stay on one runtime.
fn block<F: Future>(future: F) -> F::Output {
    tauri::async_runtime::block_on(future)
}

/// Emits `Thinking { active: false }` the first time it is called after `Thinking { active: true }`.
fn stop_thinking(ctx: &AgentCtx<'_>, active: &Cell<bool>) {
    if active.replace(false) {
        ctx.emit(AgentEvent::Thinking { active: false });
    }
}

/// The notice for a failed model call. Provider text is already free of the key (the adapter scrubs it).
fn error_text(e: &LlmError) -> String {
    match e {
        LlmError::Http { status, message, .. } => format!("The provider returned HTTP {status}: {message}"),
        LlmError::Provider { message, .. } => format!("The provider reported an error: {message}"),
        LlmError::Transport(detail) => format!("I could not reach the provider: {detail}"),
        LlmError::Parse(detail) => format!("I could not read the provider's reply: {detail}"),
        LlmError::Cancelled => "The request was cancelled.".to_string(),
    }
}

fn lock(m: &Mutex<History>) -> MutexGuard<'_, History> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn lock_string(m: &Mutex<String>) -> MutexGuard<'_, String> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A lookup over the given pairs.
    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |k: &str| map.get(k).cloned()
    }

    /// A complete set of required variables, plus `extra` (an extra value for a name replaces the base one).
    fn with_env(extra: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let mut map: HashMap<String, String> = [
            (ENV_URL, "https://openrouter.ai/api/v1/chat/completions"),
            (ENV_KEY, "sk-test-SECRETVALUE-0123456789"),
            (ENV_MODEL, "deepseek/deepseek-v4.1-flash"),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        for (k, v) in extra {
            map.insert(k.to_string(), v.to_string());
        }
        move |k: &str| map.get(k).cloned()
    }

    #[test]
    fn missing_variables_select_the_mock_agent() {
        assert!(settings_from_env(env(&[])).unwrap().is_none());
        // Any one or two of the three is still not enough.
        assert!(settings_from_env(env(&[(ENV_URL, "https://x.test/v1/chat/completions")])).unwrap().is_none());
        assert!(settings_from_env(env(&[(ENV_KEY, "sk-test-SECRETVALUE"), (ENV_MODEL, "m")])).unwrap().is_none());
        // Empty counts as missing.
        assert!(settings_from_env(with_env(&[(ENV_MODEL, "   ")])).unwrap().is_none());
    }

    #[test]
    fn all_three_variables_give_defaults_for_the_optional_ones() {
        let s = settings_from_env(with_env(&[])).unwrap().unwrap();
        assert_eq!(s.chat_url, "https://openrouter.ai/api/v1/chat/completions");
        assert_eq!(s.model, "deepseek/deepseek-v4.1-flash");
        assert!(s.stream);
        assert_eq!(s.max_tokens, None);
        assert!(s.headers.is_empty());
    }

    #[test]
    fn stream_and_max_tokens_are_parsed_and_checked() {
        assert!(!settings_from_env(with_env(&[(ENV_STREAM, "0")])).unwrap().unwrap().stream);
        assert!(settings_from_env(with_env(&[(ENV_STREAM, "1")])).unwrap().unwrap().stream);
        let err = settings_from_env(with_env(&[(ENV_STREAM, "yes")])).unwrap_err();
        assert_eq!(err, "AGENT_TERM_LLM_STREAM must be 0 or 1");
        assert_eq!(settings_from_env(with_env(&[(ENV_MAX_TOKENS, "512")])).unwrap().unwrap().max_tokens, Some(512));
        assert!(settings_from_env(with_env(&[(ENV_MAX_TOKENS, "0")])).is_err());
        assert!(settings_from_env(with_env(&[(ENV_MAX_TOKENS, "lots")])).is_err());
    }

    #[test]
    fn a_url_without_http_is_refused_without_echoing_it() {
        let err = settings_from_env(env(&[
            (ENV_URL, "ftp://user:hunter2@host/x"),
            (ENV_KEY, "sk-test-SECRETVALUE"),
            (ENV_MODEL, "m"),
        ]))
        .unwrap_err();
        assert_eq!(err, "AGENT_TERM_LLM_URL must start with https:// or http://");
    }

    #[test]
    fn headers_are_split_by_line_and_semicolon_and_the_placeholder_is_resolved() {
        let s = settings_from_env(with_env(&[(
            ENV_HEADERS,
            "x-opencode-session: {session_id}\nX-Title:  agent term ; X-Extra: a-{session_id}-b",
        )]))
        .unwrap()
        .unwrap();
        assert_eq!(
            s.headers,
            vec![
                ("x-opencode-session".to_string(), "{session_id}".to_string()),
                ("X-Title".to_string(), "agent term".to_string()),
                ("X-Extra".to_string(), "a-{session_id}-b".to_string()),
            ]
        );
        let resolved = s.resolved_headers("ses_abc");
        assert_eq!(resolved[0].1, "ses_abc");
        assert_eq!(resolved[2].1, "a-ses_abc-b");
    }

    #[test]
    fn a_bad_header_line_is_a_clear_startup_error_that_hides_the_value() {
        let err = settings_from_env(with_env(&[(
            ENV_HEADERS,
            "X-Good: 1\nthis line has the value secret-value-xyz but no colon",
        )]))
        .unwrap_err();
        assert_eq!(err, "AGENT_TERM_LLM_HEADERS entry 2 is not `Name: value`");
        assert!(!err.contains("secret-value-xyz"));

        let bad_name = settings_from_env(with_env(&[(ENV_HEADERS, "bad name: v")])).unwrap_err();
        assert!(bad_name.contains("entry 1 has an invalid header name"), "{bad_name}");
        let empty_name = settings_from_env(with_env(&[(ENV_HEADERS, ": value")])).unwrap_err();
        assert!(empty_name.contains("entry 1 is not `Name: value`"), "{empty_name}");
    }

    #[test]
    fn a_bad_header_falls_back_to_the_mock_agent() {
        let settings = settings_or_warn(with_env(&[(ENV_HEADERS, "no colon here")]));
        assert!(settings.is_none());
        assert!(settings_or_warn(with_env(&[])).is_some());
    }

    #[test]
    fn debug_output_never_shows_the_key_or_header_values() {
        let s = settings_from_env(with_env(&[(ENV_HEADERS, "x-token: header-secret-123")])).unwrap().unwrap();
        let debug = format!("{s:?}");
        assert!(!debug.contains("SECRETVALUE"), "{debug}");
        assert!(!debug.contains("header-secret-123"), "{debug}");
        assert!(debug.contains("x-token"));
    }

    #[test]
    fn conversation_ids_have_the_expected_shape_and_differ() {
        let a = new_conversation_id().unwrap();
        let b = new_conversation_id().unwrap();
        for id in [&a, &b] {
            assert_eq!(id.len(), 36, "{id}");
            assert!(id.starts_with("ses_"));
            assert!(id[4..].chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()), "{id}");
        }
        assert_ne!(a, b);
    }

    #[test]
    fn reset_clears_the_history_and_rotates_the_id() {
        let conversation = Conversation::new().unwrap();
        let before = conversation.id();
        lock(&conversation.history()).push_user("hello");
        conversation.reset().unwrap();
        assert_ne!(conversation.id(), before);
        assert!(lock(&conversation.history()).is_empty());
    }

    #[test]
    fn error_notices_name_the_status_or_the_transport_problem() {
        let http = LlmError::Http { status: 400, code: Some(400), kind: None, message: "bad model".into() };
        assert_eq!(error_text(&http), "The provider returned HTTP 400: bad model");
        assert_eq!(
            error_text(&LlmError::Transport("could not connect: refused".into())),
            "I could not reach the provider: could not connect: refused"
        );
    }
}
