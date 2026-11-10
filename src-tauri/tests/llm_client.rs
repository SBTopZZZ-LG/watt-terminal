//! Integration tests for the LLM provider adapter (core/llm) against a local fake server.
//! No network and no keys: the server is a std TcpListener on 127.0.0.1 that replays recorded fixtures
//! (tests/fixtures/llm) or inline bodies, and it hands back the raw request it received.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use agent_term_lib::core::llm::{
    AssistantReply, ChatRequest, Delta, LlmError, ProviderConfig, TurnCancel, complete, http_client, stream,
};
use serde_json::json;

const OPENROUTER_STREAM: &[u8] = include_bytes!("fixtures/llm/openrouter_stream.sse");
const OPENCODE_STREAM: &[u8] = include_bytes!("fixtures/llm/opencodego_stream.sse");
const OPENCODE_TOOLS: &[u8] = include_bytes!("fixtures/llm/opencodego_tools_nonstream.json");
const OPENROUTER_TOOLS: &[u8] = include_bytes!("fixtures/llm/openrouter_tools_nonstream.json");
const OPENROUTER_TEMPERATURE_ERROR: &[u8] = include_bytes!("fixtures/llm/openrouter_error_temperature.json");
const OPENCODE_SESSION_ERROR: &[u8] = include_bytes!("fixtures/llm/opencodego_error_missing_session.json");

/// How the fake server answers one request.
struct Script {
    status: &'static str,
    body: Vec<u8>,
    /// Stream framing: no Content-Length, connection closes at the end, body written in 1..=7 byte pieces.
    pieces: bool,
    /// Write the body (if any) and then keep the connection open without finishing it.
    stall: bool,
}

impl Script {
    fn ok(body: &[u8]) -> Self {
        Script { status: "200 OK", body: body.to_vec(), pieces: false, stall: false }
    }

    fn status(status: &'static str, body: &[u8]) -> Self {
        Script { status, body: body.to_vec(), pieces: false, stall: false }
    }

    fn sse_in_pieces(body: &[u8]) -> Self {
        Script { status: "200 OK", body: body.to_vec(), pieces: true, stall: false }
    }
}

/// Starts a one-shot fake server. Returns the chat URL and a receiver for the raw request it got.
fn serve(script: Script) -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake server");
    let addr = listener.local_addr().expect("local addr");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let Ok((mut sock, _)) = listener.accept() else { return };
        let _ = sock.set_nodelay(true);
        let raw = read_request(&mut sock);
        let _ = tx.send(raw);
        let framing = if script.pieces || script.stall {
            "Connection: close\r\n".to_string()
        } else {
            format!("Content-Length: {}\r\nConnection: close\r\n", script.body.len())
        };
        let content_type = if script.pieces { "text/event-stream" } else { "application/json" };
        let head = format!("HTTP/1.1 {}\r\nContent-Type: {content_type}\r\n{framing}\r\n", script.status);
        let _ = sock.write_all(head.as_bytes());
        if script.pieces {
            let sizes = [1usize, 2, 3, 4, 5, 6, 7];
            let mut at = 0;
            let mut i = 0;
            while at < script.body.len() {
                let end = (at + sizes[i % sizes.len()]).min(script.body.len());
                let _ = sock.write_all(&script.body[at..end]);
                at = end;
                i += 1;
            }
        } else {
            let _ = sock.write_all(&script.body);
        }
        let _ = sock.flush();
        if script.stall {
            thread::sleep(Duration::from_secs(5));
        }
    });
    (format!("http://{addr}/api/v1/chat/completions"), rx)
}

/// Reads one HTTP request (headers, then Content-Length bytes of body) and returns it as text.
fn read_request(sock: &mut std::net::TcpStream) -> String {
    let mut data = Vec::new();
    let mut buf = [0u8; 1024];
    let header_end = loop {
        if let Some(pos) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
        match sock.read(&mut buf) {
            Ok(0) | Err(_) => break data.len(),
            Ok(n) => data.extend_from_slice(&buf[..n]),
        }
    };
    let head = String::from_utf8_lossy(&data[..header_end.min(data.len())]).to_string();
    let content_length = head
        .lines()
        .filter_map(|l| l.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    while data.len() < header_end + content_length {
        match sock.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => data.extend_from_slice(&buf[..n]),
        }
    }
    String::from_utf8_lossy(&data).to_string()
}

fn cfg(url: String) -> ProviderConfig {
    ProviderConfig {
        chat_url: url,
        api_key: "test-key-not-real".to_string(),
        headers: Vec::new(),
        user_agent: "agent-term-test/1.0".to_string(),
    }
}

fn request() -> ChatRequest {
    ChatRequest {
        model: "deepseek-v4.1-flash".to_string(),
        messages: vec![json!({"role": "user", "content": "What directory am I in? Use the tool."})],
        tools: Some(vec![json!({
            "type": "function",
            "function": {
                "name": "get_cwd",
                "description": "Return the current working directory and host.",
                "parameters": {"type": "object", "properties": {}}
            }
        })]),
        max_tokens: Some(300),
    }
}

/// The request body as JSON, taken from the raw request the fake server recorded.
fn body_of(raw: &str) -> serde_json::Value {
    let body = raw.split("\r\n\r\n").nth(1).expect("request has a body");
    serde_json::from_str(body).expect("request body is JSON")
}

fn reasoning_of(deltas: &[Delta]) -> String {
    deltas
        .iter()
        .filter_map(|d| match d {
            Delta::Reasoning(s) => Some(s.as_str()),
            Delta::Text(_) => None,
        })
        .collect()
}

#[tokio::test]
async fn complete_reads_a_tool_call_from_a_non_stream_success() {
    let (url, rx) = serve(Script::ok(OPENCODE_TOOLS));
    let client = http_client().unwrap();
    let reply: AssistantReply = complete(&client, &cfg(url), &request(), &TurnCancel::new()).await.unwrap();

    assert_eq!(reply.tool_calls.len(), 1);
    assert_eq!(reply.tool_calls[0].id, "call_00_kmz9kf3kzjshdkcvbdxqkrnx");
    assert_eq!(reply.tool_calls[0].name, "get_cwd");
    assert_eq!(reply.tool_calls[0].arguments, "{}");
    assert_eq!(reply.text, "I'll check that for you.");
    assert_eq!(reply.finish_reason.as_deref(), Some("tool_calls"));
    assert_eq!(reply.usage.map(|u| u.prompt_tokens), Some(334));

    let sent = body_of(&rx.recv().unwrap());
    assert_eq!(sent["stream"], false);
    assert_eq!(sent["max_tokens"], 300);
    assert_eq!(sent["tools"][0]["function"]["name"], "get_cwd");
}

#[tokio::test]
async fn complete_tolerates_a_body_with_leading_whitespace() {
    let (url, _rx) = serve(Script::ok(OPENROUTER_TOOLS));
    let client = http_client().unwrap();
    let reply = complete(&client, &cfg(url), &request(), &TurnCancel::new()).await.unwrap();
    assert_eq!(reply.tool_calls[0].name, "get_cwd");
    assert_eq!(reply.reasoning, "The user wants me to use the get_cwd tool.");
}

#[tokio::test]
async fn stream_success_delivered_in_one_to_seven_byte_pieces_matches_the_whole_body() {
    let client = http_client().unwrap();

    let (url, rx) = serve(Script::sse_in_pieces(OPENROUTER_STREAM));
    let mut pieces_deltas = Vec::new();
    let mut on_delta = |d: Delta| pieces_deltas.push(d);
    let pieces = stream(&client, &cfg(url), &request(), &mut on_delta, &TurnCancel::new()).await.unwrap();
    let sent = body_of(&rx.recv().unwrap());
    assert_eq!(sent["stream"], true);
    assert!(sent.get("stream_options").is_none(), "stream_options is never sent");

    let (url, _rx) = serve(Script::ok(OPENROUTER_STREAM));
    let mut whole_deltas = Vec::new();
    let mut on_delta = |d: Delta| whole_deltas.push(d);
    let whole = stream(&client, &cfg(url), &request(), &mut on_delta, &TurnCancel::new()).await.unwrap();

    assert_eq!(pieces, whole, "the framing does not change the reply");
    assert_eq!(pieces_deltas, whole_deltas);
    assert_eq!(pieces.tool_calls.len(), 1);
    assert_eq!(pieces.tool_calls[0].name, "get_cwd");
    assert_eq!(pieces.tool_calls[0].arguments, "{}");
    assert_eq!(reasoning_of(&pieces_deltas), pieces.reasoning);
    assert!(!pieces.reasoning.is_empty());
    assert!(pieces.usage.is_some());
}

#[tokio::test]
async fn stream_of_the_opencode_go_fixture_in_pieces_reads_the_tool_call_and_usage() {
    let (url, _rx) = serve(Script::sse_in_pieces(OPENCODE_STREAM));
    let mut deltas = Vec::new();
    let mut on_delta = |d: Delta| deltas.push(d);
    let reply = stream(&http_client().unwrap(), &cfg(url), &request(), &mut on_delta, &TurnCancel::new())
        .await
        .unwrap();
    assert!(deltas.is_empty());
    assert_eq!(reply.tool_calls[0].id, "call_00_16a4w1l8hhiztd5311fg325f");
    assert_eq!(reply.usage.map(|u| u.completion_tokens), Some(23));
    assert_eq!(reply.raw["content"], serde_json::Value::Null);
}

#[tokio::test]
async fn http_400_with_a_nested_error_becomes_an_http_error() {
    let (url, _rx) = serve(Script::status("400 Bad Request", OPENROUTER_TEMPERATURE_ERROR));
    let err = complete(&http_client().unwrap(), &cfg(url), &request(), &TurnCancel::new()).await.unwrap_err();
    assert_eq!(
        err,
        LlmError::Http {
            status: 400,
            code: Some(400),
            kind: None,
            message: "Expected temperature to be at most 2, received 99".to_string(),
        }
    );
}

#[tokio::test]
async fn http_400_from_opencode_go_keeps_its_error_type() {
    let (url, _rx) = serve(Script::status("400 Bad Request", OPENCODE_SESSION_ERROR));
    let mut ignore = |_: Delta| {};
    let err = stream(&http_client().unwrap(), &cfg(url), &request(), &mut ignore, &TurnCancel::new())
        .await
        .unwrap_err();
    match err {
        LlmError::Http { status: 400, kind: Some(kind), .. } => assert_eq!(kind, "MissingSessionID"),
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test]
async fn two_hundred_body_with_an_error_object_is_a_provider_error() {
    let body = br#"{"error":{"message":"Provider returned error","code":502,"metadata":{"provider_name":null}}}"#;
    let (url, _rx) = serve(Script::ok(body));
    let err = complete(&http_client().unwrap(), &cfg(url), &request(), &TurnCancel::new()).await.unwrap_err();
    assert_eq!(
        err,
        LlmError::Provider { code: Some(502), kind: None, message: "Provider returned error".to_string() }
    );
}

#[tokio::test]
async fn mid_stream_error_chunk_is_a_provider_error_after_the_text_before_it() {
    let body = concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"Hel\"}}]}\n\n",
        "data: {\"error\":{\"message\":\"upstream connection dropped\",\"code\":502},",
        "\"choices\":[{\"index\":0,\"delta\":{\"content\":\"\"},\"finish_reason\":\"error\"}]}\n\n",
        "data: [DONE]\n\n",
    );
    let (url, _rx) = serve(Script::sse_in_pieces(body.as_bytes()));
    let mut deltas = Vec::new();
    let mut on_delta = |d: Delta| deltas.push(d);
    let err = stream(&http_client().unwrap(), &cfg(url), &request(), &mut on_delta, &TurnCancel::new())
        .await
        .unwrap_err();
    assert_eq!(
        err,
        LlmError::Provider { code: Some(502), kind: None, message: "upstream connection dropped".to_string() }
    );
    assert_eq!(deltas, vec![Delta::Text("Hel".to_string())]);
}

#[tokio::test]
async fn request_has_no_authorization_header_when_the_key_is_empty() {
    let (url, rx) = serve(Script::ok(OPENCODE_TOOLS));
    let mut config = cfg(url);
    config.api_key = String::new();
    complete(&http_client().unwrap(), &config, &request(), &TurnCancel::new()).await.unwrap();

    let lower = rx.recv().unwrap().to_ascii_lowercase();
    assert!(!lower.contains("authorization:"), "an empty key must send no Authorization header: {lower}");
    assert!(lower.contains("user-agent: agent-term-test/1.0"));
}

#[tokio::test]
async fn request_carries_authorization_user_agent_and_the_custom_header() {
    let (url, rx) = serve(Script::ok(OPENCODE_TOOLS));
    let mut config = cfg(url);
    config.headers.push(("x-opencode-session".to_string(), "ses_0123456789abcdef0123456789abcdef".to_string()));
    config.headers.push(("User-Agent".to_string(), "agent-term-test/2.0".to_string()));
    complete(&http_client().unwrap(), &config, &request(), &TurnCancel::new()).await.unwrap();

    let raw = rx.recv().unwrap();
    let lower = raw.to_ascii_lowercase();
    assert!(lower.starts_with("post /api/v1/chat/completions "), "{raw}");
    assert!(lower.contains("authorization: bearer test-key-not-real"));
    assert!(lower.contains("content-type: application/json"));
    assert!(lower.contains("x-opencode-session: ses_0123456789abcdef0123456789abcdef"));
    assert_eq!(lower.matches("user-agent:").count(), 1, "a caller User-Agent replaces the built-in one");
    assert!(lower.contains("user-agent: agent-term-test/2.0"));
    assert!(!lower.contains("agent-term-test/1.0"));
}

#[tokio::test]
async fn cancel_during_a_stalled_stream_returns_cancelled_within_a_second() {
    let body = b"data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n";
    let (url, _rx) = serve(Script { status: "200 OK", body: body.to_vec(), pieces: false, stall: true });
    let cancel = Arc::new(TurnCancel::new());
    let trigger = cancel.clone();
    // The cancel is issued only once the first delta has arrived, so the text before the stall is always delivered.
    let (first_tx, first_rx) = mpsc::channel::<()>();
    let (cancelled_at_tx, cancelled_at_rx) = mpsc::channel();
    thread::spawn(move || {
        // The timeout is only a safety net: if no delta ever arrives, cancel anyway and let the assertions report it.
        let _ = first_rx.recv_timeout(Duration::from_secs(10));
        let _ = cancelled_at_tx.send(Instant::now());
        trigger.cancel();
    });

    let mut deltas = Vec::new();
    let mut on_delta = |d: Delta| {
        deltas.push(d);
        let _ = first_tx.send(());
    };
    let result = stream(&http_client().unwrap(), &cfg(url), &request(), &mut on_delta, &cancel).await;
    let returned_at = Instant::now();
    let cancelled_at = cancelled_at_rx.recv().unwrap();

    assert_eq!(result.unwrap_err(), LlmError::Cancelled);
    assert!(returned_at.duration_since(cancelled_at) < Duration::from_secs(1));
    assert_eq!(deltas, vec![Delta::Text("partial".to_string())], "text before the stall was delivered");
}

#[tokio::test]
async fn stream_cut_off_mid_tool_call_without_finish_reason_is_a_transport_error() {
    // The connection closes after a partial argument string, with no finish_reason and no [DONE].
    let body = concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"run_command\",\"arguments\":\"\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"command\\\": \\\"ls\"}}]}}]}\n\n",
    );
    let (url, _rx) = serve(Script::sse_in_pieces(body.as_bytes()));
    let mut ignore = |_: Delta| {};
    let err = stream(&http_client().unwrap(), &cfg(url), &request(), &mut ignore, &TurnCancel::new())
        .await
        .unwrap_err();
    match err {
        LlmError::Transport(detail) => assert!(detail.contains("ended before the reply was complete"), "{detail}"),
        other => panic!("expected a truncated-stream transport error, got {other:?}"),
    }
}

#[tokio::test]
async fn stream_that_ends_after_a_finish_reason_without_done_is_accepted() {
    let body = concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hello\"}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
    );
    let (url, _rx) = serve(Script::sse_in_pieces(body.as_bytes()));
    let mut ignore = |_: Delta| {};
    let reply = stream(&http_client().unwrap(), &cfg(url), &request(), &mut ignore, &TurnCancel::new())
        .await
        .expect("a finish_reason makes the reply complete even without [DONE]");
    assert_eq!(reply.text, "Hello");
    assert_eq!(reply.finish_reason.as_deref(), Some("stop"));
}

/// A key the error text must never reveal, not even a prefix of it.
const SECRET: &str = "Zq7Kx9Lm2Vb4Rt8Wp3Hs6Jd1Fg5Ny0Qc";

/// An error body whose message echoes the key starting at character 490, so a 500-character cut would
/// land inside the key and leave a prefix behind if the key were removed only after truncation.
fn body_echoing_key_at_the_cut(code: u16) -> Vec<u8> {
    let message = format!("{}{}{}", "a".repeat(490), SECRET, "b".repeat(100));
    serde_json::to_vec(&json!({ "error": { "message": message, "code": code } })).unwrap()
}

fn cfg_with_secret(url: String) -> ProviderConfig {
    ProviderConfig { api_key: SECRET.to_string(), ..cfg(url) }
}

/// Asserts that no prefix of the key, four characters or longer, appears in the error text.
fn assert_no_key_prefix(err: &LlmError) {
    let shown = format!("{err} {err:?}");
    for len in 4..=SECRET.len() {
        assert!(!shown.contains(&SECRET[..len]), "prefix of {len} chars leaked: {shown}");
    }
}

#[tokio::test]
async fn key_echoed_in_a_non_stream_error_body_does_not_leak_a_prefix() {
    let (url, _rx) = serve(Script::status("400 Bad Request", &body_echoing_key_at_the_cut(400)));
    let err = complete(&http_client().unwrap(), &cfg_with_secret(url), &request(), &TurnCancel::new())
        .await
        .unwrap_err();
    assert!(matches!(err, LlmError::Http { status: 400, .. }), "{err:?}");
    assert_no_key_prefix(&err);
}

#[tokio::test]
async fn key_echoed_in_a_stream_error_body_does_not_leak_a_prefix() {
    let (url, _rx) = serve(Script::status("401 Unauthorized", &body_echoing_key_at_the_cut(401)));
    let mut ignore = |_: Delta| {};
    let err = stream(&http_client().unwrap(), &cfg_with_secret(url), &request(), &mut ignore, &TurnCancel::new())
        .await
        .unwrap_err();
    assert!(matches!(err, LlmError::Http { status: 401, .. }), "{err:?}");
    assert_no_key_prefix(&err);
}

#[tokio::test]
async fn key_echoed_in_a_200_error_body_does_not_leak_a_prefix() {
    let (url, _rx) = serve(Script::ok(&body_echoing_key_at_the_cut(502)));
    let err = complete(&http_client().unwrap(), &cfg_with_secret(url), &request(), &TurnCancel::new())
        .await
        .unwrap_err();
    assert!(matches!(err, LlmError::Provider { .. }), "{err:?}");
    assert_no_key_prefix(&err);
}

#[tokio::test]
async fn cancel_during_a_stalled_non_stream_request_returns_cancelled_promptly() {
    let (url, _rx) = serve(Script { status: "200 OK", body: Vec::new(), pieces: false, stall: true });
    let cancel = Arc::new(TurnCancel::new());
    let trigger = cancel.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(300));
        trigger.cancel();
    });
    let started = Instant::now();
    let result = complete(&http_client().unwrap(), &cfg(url), &request(), &cancel).await;
    assert_eq!(result.unwrap_err(), LlmError::Cancelled);
    assert!(started.elapsed() < Duration::from_secs(2));
}

/// Binds a fake server on 127.0.0.1 with an ephemeral port. Connection `n` gets the reply that `reply(base, n, raw)`
/// returns, a full HTTP response. Returns the base URL and the raw requests, in order. The server ends after `count`
/// connections.
fn serve_each(
    count: usize,
    reply: impl Fn(&str, usize, &str) -> String + Send + 'static,
) -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake server");
    let base = format!("http://{}", listener.local_addr().expect("local addr"));
    let (tx, rx) = mpsc::channel();
    let server_base = base.clone();
    thread::spawn(move || {
        for n in 0..count {
            let Ok((mut sock, _)) = listener.accept() else { return };
            let _ = sock.set_nodelay(true);
            let raw = read_request(&mut sock);
            let _ = tx.send(raw.clone());
            let _ = sock.write_all(reply(&server_base, n, &raw).as_bytes());
            let _ = sock.flush();
        }
    });
    (base, rx)
}

/// A complete HTTP response with `Connection: close` and the given extra header lines (each ending in CRLF).
fn http_reply(status: &str, extra_headers: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\n{extra_headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn text_reply_json(text: &str) -> String {
    json!({"choices": [{"index": 0, "finish_reason": "stop", "message": {"role": "assistant", "content": text}}]})
        .to_string()
}

#[tokio::test]
async fn a_redirect_to_the_same_origin_is_followed_and_the_body_and_headers_are_sent_again() {
    let (base, rx) = serve_each(2, |base, n, _raw| match n {
        0 => http_reply("307 Temporary Redirect", &format!("Location: {base}/moved\r\n"), ""),
        _ => http_reply("200 OK", "Content-Type: application/json\r\n", &text_reply_json("moved ok")),
    });
    let mut cfg = cfg(format!("{base}/chat"));
    cfg.headers = vec![("x-custom-key".to_string(), "custom-value-123".to_string())];
    let reply = complete(&http_client().unwrap(), &cfg, &request(), &TurnCancel::new()).await.unwrap();
    assert_eq!(reply.text, "moved ok");

    let first = rx.recv().unwrap();
    let second = rx.recv().unwrap();
    assert!(first.starts_with("POST /chat "), "{first}");
    assert!(second.starts_with("POST /moved "), "{second}");
    assert!(second.contains("What directory am I in?"), "a 307 re-sends the body to the same origin: {second}");
    assert!(second.to_ascii_lowercase().contains("x-custom-key: custom-value-123"), "{second}");
}

#[tokio::test]
async fn a_redirect_to_another_port_is_not_followed_and_the_other_server_gets_no_connection() {
    let other = TcpListener::bind("127.0.0.1:0").expect("bind other server");
    other.set_nonblocking(true).expect("non-blocking");
    let other_url = format!("http://{}/steal", other.local_addr().expect("local addr"));
    let (base, rx) = serve_each(1, move |_, _, _| {
        http_reply("307 Temporary Redirect", &format!("Location: {other_url}\r\n"), "")
    });
    let mut cfg = cfg(format!("{base}/chat"));
    cfg.headers = vec![("x-custom-key".to_string(), "custom-value-123".to_string())];

    let err = complete(&http_client().unwrap(), &cfg, &request(), &TurnCancel::new()).await.unwrap_err();
    assert_eq!(err, LlmError::Transport("the server redirected to another origin; not followed".to_string()));
    assert!(rx.recv().unwrap().starts_with("POST /chat "));
    // Nothing reached the other server: the client decided before connecting, so no connection is waiting.
    let accepted = other.accept();
    assert!(
        matches!(&accepted, Err(e) if e.kind() == std::io::ErrorKind::WouldBlock),
        "the other origin must get no connection: {accepted:?}"
    );
}

#[tokio::test]
async fn redirects_past_the_limit_fail_with_a_clear_message() {
    // The original request and ten followed redirects are eleven connections. The eleventh redirect is refused.
    let (base, _rx) = serve_each(agent_term_lib::core::llm::MAX_REDIRECTS + 1, |base, _, _| {
        http_reply("302 Found", &format!("Location: {base}/again\r\n"), "")
    });
    let err = complete(&http_client().unwrap(), &cfg(format!("{base}/chat")), &request(), &TurnCancel::new())
        .await
        .unwrap_err();
    assert_eq!(err, LlmError::Transport("too many redirects".to_string()));
}
