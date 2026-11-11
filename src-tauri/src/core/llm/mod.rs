//! Chat-completions client for OpenAI-compatible providers (OpenRouter, OpenCode Go and similar).
//!
//! Nothing here is provider-specific: a provider is data (`ProviderConfig`): the full chat URL, the key,
//! the extra headers the caller has already resolved, and the User-Agent. `complete` makes one
//! non-streaming request; `stream` makes one streaming request and reports text and reasoning deltas as they
//! arrive. Both return an `AssistantReply` whose `raw` message can be pushed straight into the history.
//!
//! Timeouts (values below): the shared client has a connect timeout only. `complete` gives the whole request
//! `COMPLETE_TIMEOUT`. `stream` waits up to `STREAM_HEADERS_TIMEOUT` for the response headers (reasoning
//! models can think for minutes before the first byte), and after that allows `STREAM_IDLE_TIMEOUT` between
//! network chunks (an idle gap, not a total). A stream that ends without `[DONE]` is accepted only if a chunk
//! carried a `finish_reason`; otherwise it is an error, because the reply was cut off. Cancelling a
//! `TurnCancel` aborts either call promptly and drops the connection.
//!
//! Sizes are capped: a non-streaming reply and any error body may be `MAX_BODY_BYTES` (32 MiB), and one SSE line or
//! event `MAX_SSE_LINE_BYTES` (1 MiB). Beyond that the call fails with `LlmError::TooLarge`, and reading stops
//! at the cap.
//!
//! Secrets: the API key is never in `Debug` output. The key is replaced with "[redacted]" in each raw body
//! (an error body, a 200 body, each stream payload) before that text is parsed or truncated, and error
//! messages get a second pass before they leave this module. Error messages are truncated to `MESSAGE_LIMIT`
//! characters.

mod accumulate;
mod sse;

use std::fmt;
use std::future::Future;
use std::pin::pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures_util::StreamExt;
use futures_util::future::{Either, select};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue, USER_AGENT};
use serde_json::{Value, json};
use tokio::sync::Notify;

use accumulate::Accumulator;
use sse::SseParser;

pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub const STREAM_HEADERS_TIMEOUT: Duration = Duration::from_secs(300);
pub const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(60);
pub const COMPLETE_TIMEOUT: Duration = Duration::from_secs(300);
const MESSAGE_LIMIT: usize = 500;

const MIB: usize = 1024 * 1024;
/// Largest reply body, in bytes, that a call reads: a non-streaming reply, an error body, and a streamed error body.
/// A bigger body fails the call, and reading stops at the cap.
pub const MAX_BODY_BYTES: usize = 32 * MIB;
/// Longest single SSE line, and longest single event's data, in bytes. A longer one fails the streamed reply.
pub const MAX_SSE_LINE_BYTES: usize = MIB;

/// The timing and size limits of one call. `DEFAULT_BOUNDS` is what the app uses; tests pass smaller sizes, so the
/// caps are exercised without moving megabytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Bounds {
    /// Wait for the response headers (and for an error body).
    headers: Duration,
    /// Gap allowed between network chunks once the headers have arrived.
    idle: Duration,
    body: usize,
    sse_line: usize,
}

const DEFAULT_BOUNDS: Bounds = Bounds {
    headers: STREAM_HEADERS_TIMEOUT,
    idle: STREAM_IDLE_TIMEOUT,
    body: MAX_BODY_BYTES,
    sse_line: MAX_SSE_LINE_BYTES,
};

/// A size as the user reads it: "32 MiB" for whole mebibytes, else bytes.
fn size_text(bytes: usize) -> String {
    if bytes >= MIB && bytes.is_multiple_of(MIB) { format!("{} MiB", bytes / MIB) } else { format!("{bytes} bytes") }
}

/// Everything the client needs to reach one provider. `headers` are already resolved by the caller
/// (a `{session_id}` placeholder is the caller's job). A header whose name matches a built-in one
/// (Authorization, Content-Type, User-Agent) replaces it.
#[derive(Clone)]
pub struct ProviderConfig {
    pub chat_url: String,
    pub api_key: String,
    pub headers: Vec<(String, String)>,
    pub user_agent: String,
}

impl fmt::Debug for ProviderConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let header_names: Vec<&str> = self.headers.iter().map(|(name, _)| name.as_str()).collect();
        f.debug_struct("ProviderConfig")
            .field("chat_url", &self.chat_url)
            .field("api_key", &"<redacted>")
            .field("header_names", &header_names)
            .field("user_agent", &self.user_agent)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Value>,
    /// Omitted from the body when `None` or empty.
    pub tools: Option<Vec<Value>>,
    /// Sent only when `Some`.
    pub max_tokens: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AssistantReply {
    /// The assistant message as it should be echoed back in history (role assistant).
    pub raw: Value,
    pub text: String,
    pub reasoning: String,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: Option<String>,
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// JSON text as the model wrote it. "{}" when the model sent no arguments.
    pub arguments: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Usage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delta {
    Text(String),
    Reasoning(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LlmError {
    /// Non-2xx HTTP status. `message` is the provider's text, or a truncated body when it is not JSON.
    Http { status: u16, code: Option<i64>, kind: Option<String>, message: String },
    /// An `error` object inside a 200 body (non-stream) or in a stream chunk.
    Provider { code: Option<i64>, kind: Option<String>, message: String },
    Transport(String),
    Parse(String),
    /// The reply is over a size cap (see `MAX_BODY_BYTES` and `MAX_SSE_LINE_BYTES`). The text says which cap.
    TooLarge(String),
    Cancelled,
}

impl fmt::Display for LlmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LlmError::Http { status, message, .. } => write!(f, "HTTP {status}: {message}"),
            LlmError::Provider { message, .. } => write!(f, "provider error: {message}"),
            LlmError::Transport(detail) => write!(f, "transport error: {detail}"),
            LlmError::Parse(detail) => write!(f, "could not parse the response: {detail}"),
            LlmError::TooLarge(detail) => write!(f, "the provider's reply is too large: {detail}"),
            LlmError::Cancelled => write!(f, "cancelled"),
        }
    }
}

impl std::error::Error for LlmError {}

/// One turn's cancel signal. Share it with `Arc`. `cancel` sets the flag and wakes everything waiting in
/// `cancelled`. Calls that already started return `Err(LlmError::Cancelled)`.
#[derive(Default)]
pub struct TurnCancel {
    flag: AtomicBool,
    notify: Notify,
}

impl TurnCancel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    /// Resolves once `cancel` has been called (immediately if it already was).
    pub async fn cancelled(&self) {
        let mut notified = pin!(self.notify.notified());
        // Register before checking the flag, so a cancel between the check and the await is not missed.
        notified.as_mut().enable();
        if self.is_cancelled() {
            return;
        }
        notified.await;
    }
}

/// Most redirects one request follows.
pub const MAX_REDIRECTS: usize = 10;

/// The shared HTTP client: rustls, connect timeout, redirects only within the origin of the request, and no default
/// User-Agent (each request sets its own).
pub fn http_client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder().connect_timeout(CONNECT_TIMEOUT).redirect(same_origin_redirects()).build()
}

/// Follows a redirect only when the new URL has the same scheme, host and port as the original request. The default
/// policy would send the key and the custom headers to another server, and would re-send a POST body on a 307 or 308.
/// Any other redirect fails the request with a message that says so, and nothing is sent to the other origin.
fn same_origin_redirects() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() > MAX_REDIRECTS {
            return attempt.error("too many redirects");
        }
        // `previous` starts with the URL of the original request.
        let same_origin = attempt.previous().first().is_some_and(|first| first.origin() == attempt.url().origin());
        if same_origin {
            attempt.follow()
        } else {
            attempt.error("the server redirected to another origin; not followed")
        }
    })
}

/// One non-streaming request (`stream: false`). Returns once the whole reply has arrived.
pub async fn complete(
    client: &reqwest::Client,
    cfg: &ProviderConfig,
    req: &ChatRequest,
    cancel: &TurnCancel,
) -> Result<AssistantReply, LlmError> {
    complete_within(client, cfg, req, cancel, DEFAULT_BOUNDS).await
}

async fn complete_within(
    client: &reqwest::Client,
    cfg: &ProviderConfig,
    req: &ChatRequest,
    cancel: &TurnCancel,
    bounds: Bounds,
) -> Result<AssistantReply, LlmError> {
    if cancel.is_cancelled() {
        return Err(LlmError::Cancelled);
    }
    let body = request_body(req, false);
    let work = async {
        let response = post(client, cfg, &body)?
            .timeout(COMPLETE_TIMEOUT)
            .send()
            .await
            .map_err(transport)?;
        let status = response.status();
        let text = redact(&read_capped(response, bounds.body).await?, &cfg.api_key);
        if !status.is_success() {
            return Err(http_error(status.as_u16(), &text));
        }
        parse_completion(&text)
    };
    race(cancel, work).await.map_err(|e| scrub(e, &cfg.api_key))
}

/// One streaming request (`stream: true`). `on_delta` is called for each text or reasoning fragment as it
/// arrives, then the full reply is returned.
pub async fn stream(
    client: &reqwest::Client,
    cfg: &ProviderConfig,
    req: &ChatRequest,
    on_delta: &mut dyn FnMut(Delta),
    cancel: &TurnCancel,
) -> Result<AssistantReply, LlmError> {
    run_stream(client, cfg, req, on_delta, cancel, DEFAULT_BOUNDS).await
}

async fn run_stream(
    client: &reqwest::Client,
    cfg: &ProviderConfig,
    req: &ChatRequest,
    on_delta: &mut dyn FnMut(Delta),
    cancel: &TurnCancel,
    bounds: Bounds,
) -> Result<AssistantReply, LlmError> {
    if cancel.is_cancelled() {
        return Err(LlmError::Cancelled);
    }
    let body = request_body(req, true);
    let work = async {
        let request = post(client, cfg, &body)?;
        let response = with_idle(bounds.headers, request.send()).await?.map_err(transport)?;
        let status = response.status();
        if !status.is_success() {
            let text = with_idle(bounds.headers, read_capped(response, bounds.body)).await??;
            return Err(http_error(status.as_u16(), &redact(&text, &cfg.api_key)));
        }
        let mut chunks = pin!(response.bytes_stream());
        let mut parser = SseParser::new(bounds.sse_line);
        let mut acc = Accumulator::new();
        let mut done = false;
        while !done {
            match with_idle(bounds.idle, chunks.next()).await? {
                None => break,
                Some(bytes) => {
                    let bytes = bytes.map_err(transport)?;
                    done = feed(parser.push(&bytes)?, &cfg.api_key, &mut acc, on_delta)?;
                }
            }
        }
        if !done {
            feed(parser.finish()?, &cfg.api_key, &mut acc, on_delta)?;
        }
        // Without [DONE], a finish_reason means the reply is complete (some providers omit [DONE]). Without
        // either, the connection closed mid-reply.
        if !done && !acc.has_finish_reason() {
            return Err(LlmError::Transport("the stream ended before the reply was complete".to_string()));
        }
        acc.finish()
    };
    race(cancel, work).await.map_err(|e| scrub(e, &cfg.api_key))
}

/// Feeds parsed SSE payloads to the accumulator. Each payload has the key redacted before it is parsed.
/// Returns true once `[DONE]` has been seen.
fn feed(
    payloads: Vec<String>,
    secret: &str,
    acc: &mut Accumulator,
    on_delta: &mut dyn FnMut(Delta),
) -> Result<bool, LlmError> {
    for payload in payloads {
        let payload = redact(&payload, secret);
        if payload.trim() == "[DONE]" {
            return Ok(true);
        }
        let chunk: Value = serde_json::from_str(&payload)
            .map_err(|e| LlmError::Parse(format!("stream chunk is not JSON ({e}): {}", truncate(&payload))))?;
        let mut deltas = Vec::new();
        acc.push_chunk(&chunk, &mut deltas)?;
        for delta in deltas {
            on_delta(delta);
        }
    }
    Ok(false)
}

/// Runs `work` unless `cancel` fires first. Dropping `work` drops the connection.
async fn race<T>(cancel: &TurnCancel, work: impl Future<Output = Result<T, LlmError>>) -> Result<T, LlmError> {
    match select(pin!(work), pin!(cancel.cancelled())).await {
        Either::Left((result, _)) => result,
        Either::Right(((), _)) => Err(LlmError::Cancelled),
    }
}

async fn with_idle<F: Future>(idle: Duration, fut: F) -> Result<F::Output, LlmError> {
    tokio::time::timeout(idle, fut).await.map_err(|_| {
        LlmError::Transport(format!("no data from the provider for {} s", idle.as_secs_f64()))
    })
}

/// Reads the whole body as text, up to `max` bytes. Reading stops as soon as the cap is passed, so a huge body is
/// never held in memory. Invalid UTF-8 is replaced, since provider JSON is UTF-8.
async fn read_capped(response: reqwest::Response, max: usize) -> Result<String, LlmError> {
    let mut chunks = pin!(response.bytes_stream());
    let mut body: Vec<u8> = Vec::new();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(transport)?;
        if body.len().saturating_add(chunk.len()) > max {
            return Err(LlmError::TooLarge(format!("more than {}", size_text(max))));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8_lossy(&body).into_owned())
}

fn post(client: &reqwest::Client, cfg: &ProviderConfig, body: &Value) -> Result<reqwest::RequestBuilder, LlmError> {
    Ok(client.post(&cfg.chat_url).headers(build_headers(cfg)?).json(body))
}

/// Authorization (only when there is a key), Content-Type and User-Agent first, then the caller's headers. A caller
/// header with the same name replaces the built-in one.
pub(crate) fn build_headers(cfg: &ProviderConfig) -> Result<HeaderMap, LlmError> {
    let mut headers = HeaderMap::new();
    // A local server may need no key at all. An empty key sends no Authorization header.
    if !cfg.api_key.is_empty() {
        let bearer = HeaderValue::from_str(&format!("Bearer {}", cfg.api_key)).map_err(|_| {
            LlmError::Transport("the API key has characters that are not allowed in a header".to_string())
        })?;
        headers.insert(AUTHORIZATION, bearer);
    }
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    let user_agent = HeaderValue::from_str(&cfg.user_agent)
        .map_err(|_| LlmError::Transport("the User-Agent is not a valid header value".to_string()))?;
    headers.insert(USER_AGENT, user_agent);
    for (name, value) in &cfg.headers {
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| LlmError::Transport(format!("invalid header name {name:?}")))?;
        let value = HeaderValue::from_str(value)
            .map_err(|_| LlmError::Transport(format!("invalid value for header {name}")))?;
        headers.insert(name, value);
    }
    Ok(headers)
}

fn request_body(req: &ChatRequest, stream: bool) -> Value {
    let mut body = json!({ "model": req.model, "messages": req.messages, "stream": stream });
    if let Some(tools) = req.tools.as_ref().filter(|t| !t.is_empty()) {
        body["tools"] = Value::Array(tools.clone());
    }
    if let Some(max_tokens) = req.max_tokens {
        body["max_tokens"] = json!(max_tokens);
    }
    body
}

pub(crate) fn transport(err: reqwest::Error) -> LlmError {
    // The URL is dropped: a configured endpoint may carry a token in its userinfo or query.
    let err = err.without_url();
    if err.is_redirect() {
        // The redirect policy's own message (see `same_origin_redirects`) is the cause. reqwest's text is generic.
        let cause = std::error::Error::source(&err).map_or_else(|| err.to_string(), ToString::to_string);
        LlmError::Transport(cause)
    } else if err.is_timeout() {
        LlmError::Transport("the request timed out".to_string())
    } else if err.is_connect() {
        LlmError::Transport(format!("could not connect: {err}"))
    } else {
        LlmError::Transport(format!("request failed: {err}"))
    }
}

fn parse_completion(text: &str) -> Result<AssistantReply, LlmError> {
    // Providers sometimes lead the body with blank lines or spaces (seen on OpenRouter).
    let body: Value = serde_json::from_str(text.trim_start())
        .map_err(|e| LlmError::Parse(format!("body is not JSON ({e}): {}", truncate(text))))?;
    let has_choices = body.get("choices").and_then(Value::as_array).is_some_and(|c| !c.is_empty());
    if !has_choices
        && let Some(err) = body.get("error").filter(|e| !e.is_null())
    {
        return Err(provider_error(err));
    }
    accumulate::from_completion(&body)
}

fn http_error(status: u16, body: &str) -> LlmError {
    let parsed: Option<Value> = serde_json::from_str(body.trim_start()).ok();
    match parsed.as_ref().and_then(|v| v.get("error")).filter(|e| !e.is_null()) {
        Some(err) => {
            let (code, kind, message) = error_fields(err);
            LlmError::Http { status, code, kind, message }
        }
        None => {
            let text = truncate(body.trim());
            let message = if text.is_empty() { format!("HTTP {status} with an empty body") } else { text };
            LlmError::Http { status, code: None, kind: None, message }
        }
    }
}

/// An `error` object from an error status, a 200 body or a stream chunk.
/// Handles `{message, code, metadata?}` (OpenRouter) and `{type, message}` (OpenCode Go).
fn provider_error(err: &Value) -> LlmError {
    let (code, kind, message) = error_fields(err);
    LlmError::Provider { code, kind, message }
}

fn error_fields(err: &Value) -> (Option<i64>, Option<String>, String) {
    match err {
        Value::String(text) => (None, None, truncate(text)),
        Value::Object(_) => {
            let code = err.get("code").and_then(|c| c.as_i64().or_else(|| c.as_str()?.parse().ok()));
            let kind = err.get("type").and_then(Value::as_str).map(str::to_string);
            let message = err
                .get("message")
                .and_then(Value::as_str)
                .map(truncate)
                .unwrap_or_else(|| "the provider returned an error without a message".to_string());
            (code, kind, message)
        }
        _ => (None, None, "the provider returned an error".to_string()),
    }
}

fn truncate(text: &str) -> String {
    if text.chars().count() <= MESSAGE_LIMIT {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(MESSAGE_LIMIT).collect();
    cut.push_str("...");
    cut
}

/// Replaces the key in a raw body or payload. Runs before parsing and truncation, so a key that would
/// straddle the message cut is removed whole. An empty key replaces nothing.
pub(crate) fn redact(text: &str, secret: &str) -> String {
    if secret.is_empty() { text.to_string() } else { text.replace(secret, "[redacted]") }
}

fn scrub(err: LlmError, secret: &str) -> LlmError {
    if secret.is_empty() {
        return err;
    }
    let clean = |s: String| if s.contains(secret) { s.replace(secret, "[redacted]") } else { s };
    match err {
        LlmError::Http { status, code, kind, message } => {
            LlmError::Http { status, code, kind: kind.map(clean), message: clean(message) }
        }
        LlmError::Provider { code, kind, message } => {
            LlmError::Provider { code, kind: kind.map(clean), message: clean(message) }
        }
        LlmError::Transport(detail) => LlmError::Transport(clean(detail)),
        LlmError::Parse(detail) => LlmError::Parse(clean(detail)),
        LlmError::TooLarge(detail) => LlmError::TooLarge(clean(detail)),
        LlmError::Cancelled => LlmError::Cancelled,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(url: &str) -> ProviderConfig {
        ProviderConfig {
            chat_url: url.to_string(),
            api_key: "test-key-123".to_string(),
            headers: Vec::new(),
            user_agent: "watt-terminal-test/0".to_string(),
        }
    }

    fn one_message() -> ChatRequest {
        ChatRequest {
            model: "m".to_string(),
            messages: vec![json!({"role": "user", "content": "hi"})],
            tools: None,
            max_tokens: None,
        }
    }

    /// Accepts one connection, sends `head` and `body`, then holds the socket open for a few seconds.
    fn stalling_server(head: &'static str, body: &'static str) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let _ = sock.read(&mut buf);
            let _ = sock.write_all(head.as_bytes());
            let _ = sock.write_all(body.as_bytes());
            let _ = sock.flush();
            std::thread::sleep(Duration::from_secs(3));
        });
        format!("http://{addr}/v1/chat/completions")
    }

    #[test]
    fn request_body_omits_tools_when_none_and_sends_max_tokens_only_when_set() {
        let body = request_body(&one_message(), false);
        assert_eq!(body["stream"], false);
        assert!(body.get("tools").is_none());
        assert!(body.get("max_tokens").is_none());
        assert!(body.get("stream_options").is_none());

        let req = ChatRequest { tools: Some(vec![json!({"type": "function"})]), max_tokens: Some(64), ..one_message() };
        let body = request_body(&req, true);
        assert_eq!(body["stream"], true);
        assert_eq!(body["max_tokens"], 64);
        assert_eq!(body["tools"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn caller_header_replaces_a_built_in_one_and_debug_hides_the_key() {
        let mut c = cfg("http://x/");
        c.headers.push(("user-agent".to_string(), "custom/1".to_string()));
        c.headers.push(("x-opencode-session".to_string(), "ses_abc".to_string()));
        let headers = build_headers(&c).unwrap();
        assert_eq!(headers.get(AUTHORIZATION).unwrap(), "Bearer test-key-123");
        assert_eq!(headers.get(USER_AGENT).unwrap(), "custom/1");
        assert_eq!(headers.get("x-opencode-session").unwrap(), "ses_abc");
        assert_eq!(headers.get_all(USER_AGENT).iter().count(), 1);
        let debug = format!("{c:?}");
        assert!(!debug.contains("test-key-123"));
        assert!(debug.contains("x-opencode-session"));
        assert!(!debug.contains("ses_abc"));
    }

    #[test]
    fn scrub_removes_the_key_from_error_text() {
        let err = LlmError::Transport("bad Bearer test-key-123 here".to_string());
        assert_eq!(scrub(err, "test-key-123"), LlmError::Transport("bad Bearer [redacted] here".to_string()));
    }

    #[test]
    fn long_messages_are_cut_to_the_limit() {
        let long = "x".repeat(2000);
        assert_eq!(truncate(&long).chars().count(), MESSAGE_LIMIT + 3);
        assert_eq!(truncate("short"), "short");
    }

    #[test]
    fn openrouter_400_body_gives_status_code_and_message() {
        let body = include_str!("../../../tests/fixtures/llm/openrouter_error_temperature.json");
        assert_eq!(
            http_error(400, body),
            LlmError::Http {
                status: 400,
                code: Some(400),
                kind: None,
                message: "Expected temperature to be at most 2, received 99".to_string(),
            }
        );
    }

    #[test]
    fn openrouter_unknown_model_body_gives_status_code_and_message() {
        let body = include_str!("../../../tests/fixtures/llm/openrouter_error_model.json");
        assert_eq!(
            http_error(400, body),
            LlmError::Http {
                status: 400,
                code: Some(400),
                kind: None,
                message: "deepseek/no-such-model-xyz is not a valid model ID".to_string(),
            }
        );
    }

    #[test]
    fn opencode_go_error_type_and_message_are_read_from_the_nested_error() {
        let body = include_str!("../../../tests/fixtures/llm/opencodego_error_missing_session.json");
        match http_error(400, body) {
            LlmError::Http { status: 400, code: None, kind: Some(kind), message } => {
                assert_eq!(kind, "MissingSessionID");
                assert!(message.starts_with("Request is missing x-opencode-session"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn non_json_error_body_falls_back_to_truncated_text() {
        let html = "<html>".repeat(200);
        match http_error(502, &html) {
            LlmError::Http { status: 502, message, .. } => assert!(message.chars().count() <= MESSAGE_LIMIT + 3),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn non_stream_body_tolerates_leading_whitespace() {
        let body = include_str!("../../../tests/fixtures/llm/openrouter_tools_nonstream.json");
        assert!(body.starts_with(char::is_whitespace));
        let reply = parse_completion(body).unwrap();
        assert_eq!(reply.tool_calls[0].name, "get_cwd");
    }

    #[test]
    fn two_hundred_body_with_error_and_no_choices_is_a_provider_error() {
        let body = r#"{"error":{"message":"Provider returned error","code":429,"metadata":{"provider_name":null}}}"#;
        assert_eq!(
            parse_completion(body).unwrap_err(),
            LlmError::Provider { code: Some(429), kind: None, message: "Provider returned error".to_string() }
        );
    }

    #[test]
    fn stream_chunks_drive_the_callback_and_the_final_reply() {
        let raw = include_str!("../../../tests/fixtures/llm/openrouter_stream.sse");
        let mut acc = Accumulator::new();
        let mut parser = SseParser::new(MAX_SSE_LINE_BYTES);
        let mut seen = Vec::new();
        let mut on_delta = |d: Delta| seen.push(d);
        let done = feed(parser.push(raw.as_bytes()).unwrap(), "", &mut acc, &mut on_delta).unwrap();
        assert!(done);
        drop(on_delta);
        assert!(seen.iter().any(|d| matches!(d, Delta::Reasoning(_))));
        assert_eq!(acc.finish().unwrap().tool_calls[0].name, "get_cwd");
    }

    #[test]
    fn cancel_before_the_call_returns_cancelled_without_connecting() {
        let cancel = TurnCancel::new();
        cancel.cancel();
        let client = http_client().unwrap();
        let mut ignore = |_d: Delta| {};
        let result = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(stream(&client, &cfg("http://127.0.0.1:9/never"), &one_message(), &mut ignore, &cancel));
        assert_eq!(result.unwrap_err(), LlmError::Cancelled);
    }

    #[test]
    fn stalled_stream_hits_the_idle_timeout() {
        let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";
        let body = "data: {\"choices\":[{\"delta\":{\"content\":\"hel\"}}]}\n\n";
        let url = stalling_server(head, body);
        let client = http_client().unwrap();
        let cancel = TurnCancel::new();
        let mut seen = String::new();
        let mut on_delta = |d: Delta| {
            if let Delta::Text(t) = d {
                seen.push_str(&t);
            }
        };
        let bounds = Bounds { idle: Duration::from_millis(300), ..DEFAULT_BOUNDS };
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(run_stream(&client, &cfg(&url), &one_message(), &mut on_delta, &cancel, bounds));
        match result {
            Err(LlmError::Transport(detail)) => assert!(detail.contains("no data"), "{detail}"),
            other => panic!("expected an idle timeout, got {other:?}"),
        }
        assert_eq!(seen, "hel", "the text before the stall was still delivered");
    }

    #[test]
    fn silent_server_hits_the_headers_timeout_not_the_idle_one() {
        // Accepts the request and sends nothing at all.
        let url = stalling_server("", "");
        let client = http_client().unwrap();
        let cancel = TurnCancel::new();
        let mut ignore = |_d: Delta| {};
        let bounds = Bounds { headers: Duration::from_millis(300), ..DEFAULT_BOUNDS };
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(run_stream(&client, &cfg(&url), &one_message(), &mut ignore, &cancel, bounds));
        match result {
            // The detail names the 0.3 s headers limit; the 60 s idle limit would have said 60 s.
            Err(LlmError::Transport(detail)) => assert!(detail.contains("0.3 s"), "{detail}"),
            other => panic!("expected a headers timeout, got {other:?}"),
        }
    }

    /// Bodies and SSE lines of 1 KiB, so the size tests stay small.
    const SMALL_BOUNDS: Bounds = Bounds { body: 1024, sse_line: 1024, ..DEFAULT_BOUNDS };

    fn block_on<T>(fut: impl Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(fut)
    }

    fn http_response(status: &str, content_type: &str, body: &[u8]) -> Vec<u8> {
        let mut out = format!(
            "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        out.extend_from_slice(body);
        out
    }

    /// Accepts one connection, reads the whole request (headers and the Content-Length body), sends `response` and
    /// closes. Reading the whole request first means the close does not reset the connection under the client.
    fn fixed_server(response: Vec<u8>) -> String {
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            read_request(&mut sock);
            let _ = sock.write_all(&response);
            let _ = sock.flush();
        });
        format!("http://{addr}/v1/chat/completions")
    }

    fn read_request(sock: &mut std::net::TcpStream) {
        use std::io::Read;
        let mut data = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let n = match sock.read(&mut buf) {
                Ok(0) | Err(_) => return,
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
                return;
            }
        }
    }

    #[test]
    fn the_caps_are_32_mib_for_a_body_and_1_mib_for_a_line() {
        assert_eq!(MAX_BODY_BYTES, 32 * 1024 * 1024);
        assert_eq!(MAX_SSE_LINE_BYTES, 1024 * 1024);
        assert_eq!(size_text(MAX_BODY_BYTES), "32 MiB");
        assert_eq!(size_text(1000), "1000 bytes");
    }

    #[test]
    fn a_non_streaming_reply_at_the_body_cap_is_read_and_one_over_it_fails() {
        let client = http_client().unwrap();
        let cancel = TurnCancel::new();
        let json = r#"{"choices":[{"message":{"role":"assistant","content":"hi"}}]}"#;
        let at_cap = Bounds { body: json.len(), ..DEFAULT_BOUNDS };
        let url = fixed_server(http_response("200 OK", "application/json", json.as_bytes()));
        let reply = block_on(complete_within(&client, &cfg(&url), &one_message(), &cancel, at_cap)).unwrap();
        assert_eq!(reply.text, "hi");

        let padded =
            format!(r#"{{"choices":[{{"message":{{"role":"assistant","content":"{}"}}}}]}}"#, "x".repeat(2048));
        let url = fixed_server(http_response("200 OK", "application/json", padded.as_bytes()));
        let err = block_on(complete_within(&client, &cfg(&url), &one_message(), &cancel, SMALL_BOUNDS)).unwrap_err();
        assert!(matches!(err, LlmError::TooLarge(_)), "{err:?}");
        assert_eq!(err.to_string(), "the provider's reply is too large: more than 1024 bytes");
    }

    #[test]
    fn a_streamed_error_body_over_the_cap_fails_as_too_large() {
        let body = format!(r#"{{"error":{{"message":"{}"}}}}"#, "y".repeat(2048));
        let url = fixed_server(http_response("500 Internal Server Error", "application/json", body.as_bytes()));
        let client = http_client().unwrap();
        let cancel = TurnCancel::new();
        let mut ignore = |_d: Delta| {};
        let err = block_on(run_stream(&client, &cfg(&url), &one_message(), &mut ignore, &cancel, SMALL_BOUNDS))
            .unwrap_err();
        assert!(matches!(err, LlmError::TooLarge(_)), "{err:?}");
    }

    #[test]
    fn a_stream_line_over_the_cap_fails_the_turn() {
        let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";
        let long = format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"{}\"}}}}]}}\n\n", "z".repeat(2048));
        let mut response = head.as_bytes().to_vec();
        response.extend_from_slice(long.as_bytes());
        let url = fixed_server(response);
        let client = http_client().unwrap();
        let cancel = TurnCancel::new();
        let mut ignore = |_d: Delta| {};
        let err = block_on(run_stream(&client, &cfg(&url), &one_message(), &mut ignore, &cancel, SMALL_BOUNDS))
            .unwrap_err();
        assert!(matches!(err, LlmError::TooLarge(_)), "{err:?}");
        assert!(err.to_string().contains("single line or event"), "{err}");
    }
}
