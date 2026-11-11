//! Integration tests for the model-list fetch (core/settings/models) against a local fake server. No network and no
//! keys: the server is a std TcpListener on 127.0.0.1 that replays one canned answer and hands back the raw request.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::thread;

use watt_terminal_lib::core::llm::http_client;
use watt_terminal_lib::core::settings::models::{FetchedModel, ModelsError, fetch_models};

const KEY: &str = "sk-or-test-DUMMY-fetch-key-0123456789";
const USER_AGENT: &str = "watt-terminal-test/3.0";

/// Starts a one-shot fake server that answers with `status` and `body`. Returns the models URL and a receiver for
/// the raw request it got.
fn serve(status: &'static str, body: Vec<u8>) -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake server");
    let addr = listener.local_addr().expect("local addr");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let Ok((mut sock, _)) = listener.accept() else { return };
        let _ = tx.send(read_request(&mut sock));
        let head = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = sock.write_all(head.as_bytes());
        // A client that stops reading (the oversized case) makes this fail. That is expected, so the error is ignored.
        let _ = sock.write_all(&body);
        let _ = sock.flush();
    });
    (format!("http://{addr}/api/v1/models"), rx)
}

/// Reads one HTTP request head (a GET has no body) and returns it as text.
fn read_request(sock: &mut std::net::TcpStream) -> String {
    let mut data = Vec::new();
    let mut buf = [0u8; 1024];
    loop {
        if data.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        match sock.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => data.extend_from_slice(&buf[..n]),
        }
    }
    String::from_utf8_lossy(&data).to_string()
}

fn headers(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

#[tokio::test]
async fn success_parses_the_list_and_sends_the_bearer_and_custom_header() {
    let body = br#"{"data":[
        {"id":"deepseek/deepseek-v4.1-flash","context_length":1048576,"top_provider":{"context_length":1048576}},
        {"id":"other/model","context_length":null,"top_provider":{"context_length":8192}}
    ]}"#;
    let (url, rx) = serve("200 OK", body.to_vec());
    let models = fetch_models(
        &http_client().unwrap(),
        &url,
        KEY,
        &headers(&[("x-opencode-session", "ses_0123456789abcdef0123456789abcdef")]),
        USER_AGENT,
    )
    .await
    .unwrap();

    assert_eq!(
        models,
        vec![
            FetchedModel { id: "deepseek/deepseek-v4.1-flash".into(), context: Some(1_048_576) },
            FetchedModel { id: "other/model".into(), context: Some(8192) },
        ]
    );

    let raw = rx.recv().unwrap();
    let lower = raw.to_ascii_lowercase();
    assert!(lower.starts_with("get /api/v1/models "), "{raw}");
    assert!(lower.contains("authorization: "), "{raw}");
    assert!(raw.contains(&format!("Bearer {KEY}")), "the bearer carries the key exactly");
    assert!(lower.contains("x-opencode-session: ses_0123456789abcdef0123456789abcdef"));
    assert!(lower.contains(&format!("user-agent: {}", USER_AGENT.to_ascii_lowercase())));
}

#[tokio::test]
async fn an_unauthorized_json_body_becomes_an_http_error_without_the_key() {
    // The provider echoes the key back in its message. It must not reach the error.
    let body = format!(r#"{{"error":{{"message":"No auth credentials found for {KEY}","code":401}}}}"#);
    let (url, _rx) = serve("401 Unauthorized", body.into_bytes());
    let err = fetch_models(&http_client().unwrap(), &url, KEY, &[], USER_AGENT).await.unwrap_err();

    match &err {
        ModelsError::Http { status, message } => {
            assert_eq!(*status, 401);
            assert_eq!(message, "No auth credentials found for [redacted]");
        }
        other => panic!("expected an HTTP error, got {other:?}"),
    }
    assert!(!format!("{err} {err:?}").contains(KEY));
}

#[tokio::test]
async fn a_body_over_five_mebibytes_is_refused() {
    // Valid JSON, but longer than the cap. The client must stop at the cap and report it, not parse it.
    let mut body = br#"{"data":[{"id":"a"}],"padding":""#.to_vec();
    body.resize(6 * 1024 * 1024, b'x');
    body.extend_from_slice(b"\"}");
    let (url, _rx) = serve("200 OK", body);
    let err = fetch_models(&http_client().unwrap(), &url, KEY, &[], USER_AGENT).await.unwrap_err();
    assert_eq!(err, ModelsError::TooLarge);
}

#[tokio::test]
async fn an_empty_key_sends_no_bearer_header() {
    let (url, rx) = serve("200 OK", br#"{"data":[{"id":"local-model"}]}"#.to_vec());
    let models = fetch_models(&http_client().unwrap(), &url, "", &[], USER_AGENT).await.unwrap();
    assert_eq!(models, vec![FetchedModel { id: "local-model".into(), context: None }]);

    let lower = rx.recv().unwrap().to_ascii_lowercase();
    assert!(!lower.contains("authorization:"), "an empty key must send no Authorization header: {lower}");
    assert!(lower.contains(&format!("user-agent: {}", USER_AGENT.to_ascii_lowercase())));
}

#[tokio::test]
async fn a_success_body_that_is_not_a_model_list_is_a_parse_error() {
    let (url, _rx) = serve("200 OK", br#"{"message":"hello"}"#.to_vec());
    let err = fetch_models(&http_client().unwrap(), &url, KEY, &[], USER_AGENT).await.unwrap_err();
    assert!(matches!(err, ModelsError::Parse(_)), "{err:?}");
}

#[tokio::test]
async fn a_redirect_to_another_port_is_not_followed_and_the_other_server_gets_no_connection() {
    // The key and the custom header must not follow a redirect to another origin.
    let other = TcpListener::bind("127.0.0.1:0").expect("bind other server");
    other.set_nonblocking(true).expect("non-blocking");
    let other_url = format!("http://{}/api/v1/models", other.local_addr().expect("local addr"));
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake server");
    let url = format!("http://{}/api/v1/models", listener.local_addr().expect("local addr"));
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let Ok((mut sock, _)) = listener.accept() else { return };
        let _ = tx.send(read_request(&mut sock));
        let head = format!(
            "HTTP/1.1 307 Temporary Redirect\r\nLocation: {other_url}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
        let _ = sock.write_all(head.as_bytes());
        let _ = sock.flush();
    });

    let custom = headers(&[("x-custom-key", "custom-value-123")]);
    let err = fetch_models(&http_client().unwrap(), &url, KEY, &custom, USER_AGENT).await.unwrap_err();
    assert_eq!(err, ModelsError::Transport("the server redirected to another origin; not followed".to_string()));
    let raw = rx.recv().unwrap();
    assert!(raw.to_ascii_lowercase().starts_with("get /api/v1/models "), "{raw}");
    let accepted = other.accept();
    assert!(
        matches!(&accepted, Err(e) if e.kind() == std::io::ErrorKind::WouldBlock),
        "the other origin must get no connection: {accepted:?}"
    );
}
