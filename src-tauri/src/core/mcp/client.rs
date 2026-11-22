//! Connects one MCP server: starts the stdio process (or opens the Streamable HTTP client), runs the initialize handshake
//! and lists its tools, with a connect timeout. Used on the runtime thread only (core/mcp/runtime); a connected client
//! is returned as a `Live` for the runtime to keep and close.

use std::collections::{BTreeMap, HashMap};
use std::process::Stdio;
use std::time::Duration;

use process_wrap::tokio::CommandWrap;
use reqwest::header::{HeaderName, HeaderValue};
use rmcp::model::Tool;
use rmcp::service::{ClientInitializeError, NotificationContext, Peer, RunningService};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::{StreamableHttpClientTransport, TokioChildProcess};
use rmcp::{ClientHandler, RoleClient, ServiceExt};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::ChildStderr;
use tokio::sync::mpsc::UnboundedSender;

#[cfg(windows)]
use process_wrap::tokio::JobObject;
#[cfg(unix)]
use process_wrap::tokio::ProcessGroup;

use super::expand::ResolvedTransport;
use super::launch::resolve_launch;
use super::runtime::Msg;

/// How long a server gets to start, initialize and list its tools. Cold `npx` and `uvx` starts can take 10 s or more.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(60);
/// How long a graceful close may take before the process tree is killed.
pub const CLOSE_TIMEOUT: Duration = Duration::from_secs(3);
/// The text shown when a server needs credentials the app does not have (OAuth is not implemented yet).
pub const AUTH_MESSAGE: &str = "server requires authentication: add a key in headers (OAuth login is not available yet)";

/// Why a connect attempt failed. The text is not yet redacted: the runtime does that before it stores it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectFailure {
    Failed(String),
    NeedsAuth(String),
}

/// A connected client. Owned by the runtime; closed on the runtime thread.
pub struct Live {
    pub service: RunningService<RoleClient, Handler>,
    pub peer: Peer<RoleClient>,
}

/// The rmcp client handler. It only reports events to the runtime: a list change asks for a fresh `tools/list`.
pub struct Handler {
    pub server: String,
    pub attempt: u64,
    pub events: UnboundedSender<Msg>,
}

impl ClientHandler for Handler {
    async fn on_tool_list_changed(&self, _context: NotificationContext<RoleClient>) {
        let _ = self.events.send(Msg::ToolListChanged { server: self.server.clone(), attempt: self.attempt });
    }
}

/// Connects `transport` for `server` and lists its tools. Stderr of a stdio server goes to the runtime as `Msg::Log`.
pub async fn connect(
    server: &str,
    attempt: u64,
    transport: ResolvedTransport,
    events: UnboundedSender<Msg>,
) -> Result<(Live, Vec<Tool>), ConnectFailure> {
    match tokio::time::timeout(CONNECT_TIMEOUT, connect_inner(server, attempt, transport, events)).await {
        Ok(result) => result,
        Err(_) => Err(ConnectFailure::Failed(format!(
            "did not finish connecting within {} s",
            CONNECT_TIMEOUT.as_secs()
        ))),
    }
}

async fn connect_inner(
    server: &str,
    attempt: u64,
    transport: ResolvedTransport,
    events: UnboundedSender<Msg>,
) -> Result<(Live, Vec<Tool>), ConnectFailure> {
    let handler = Handler { server: server.to_string(), attempt, events: events.clone() };
    match transport {
        ResolvedTransport::Stdio { command, args, cwd, env } => {
            let (process, stderr) = spawn_stdio(&command, args, cwd, env).map_err(ConnectFailure::Failed)?;
            if let Some(stderr) = stderr {
                drain_stderr(server.to_string(), stderr, events);
            }
            let service = handler.serve(process).await.map_err(|e| init_failure(&e, false))?;
            finish(service, false).await
        }
        ResolvedTransport::Http { url, headers } => {
            let config = http_config(&url, &headers).map_err(ConnectFailure::Failed)?;
            let service = handler
                .serve(StreamableHttpClientTransport::from_config(config))
                .await
                .map_err(|e| init_failure(&e, true))?;
            finish(service, true).await
        }
    }
}

/// Starts the stdio server. The process is wrapped so that its whole tree is killed with it: a Job Object on Windows
/// (killed when the job closes), a process group elsewhere. Stderr is piped so it can be drained.
fn spawn_stdio(
    command: &str,
    args: Vec<String>,
    cwd: Option<String>,
    env: BTreeMap<String, String>,
) -> Result<(TokioChildProcess, Option<ChildStderr>), String> {
    let program = resolve_launch(command, cfg!(windows));
    let mut wrapped = CommandWrap::with_new(program, move |c| {
        c.args(&args);
        if let Some(cwd) = &cwd {
            c.current_dir(cwd);
        }
        // The server's own values go on top of the inherited environment (no env_clear).
        c.envs(&env);
    });
    #[cfg(windows)]
    wrapped.wrap(JobObject);
    #[cfg(unix)]
    wrapped.wrap(ProcessGroup::leader());
    TokioChildProcess::builder(wrapped)
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not start the server process: {e}"))
}

/// The most bytes of one stderr line that are kept. A longer line keeps its first bytes; the rest is read and dropped.
const STDERR_LINE_BYTES: u64 = 8 * 1024;

/// Reads one stderr line into `buf`, keeping at most `STDERR_LINE_BYTES` of it. Past the cap the line is read and dropped
/// up to its newline, so the pipe keeps draining and memory stays bounded. False at the end of the stream.
async fn read_capped_line<R: AsyncBufRead + Unpin>(reader: &mut R, buf: &mut Vec<u8>) -> std::io::Result<bool> {
    buf.clear();
    let n = (&mut *reader).take(STDERR_LINE_BYTES).read_until(b'\n', buf).await?;
    if n == 0 {
        return Ok(false);
    }
    if !buf.ends_with(b"\n") && buf.len() as u64 == STDERR_LINE_BYTES {
        let mut rest = Vec::new();
        loop {
            rest.clear();
            let m = (&mut *reader).take(STDERR_LINE_BYTES).read_until(b'\n', &mut rest).await?;
            if m == 0 || rest.ends_with(b"\n") {
                break;
            }
        }
    }
    Ok(true)
}

/// Reads the server's stderr line by line for the rest of its life, even when nobody reads the log: an undrained pipe
/// could block the server. A line that is not UTF-8 is kept with replacement characters, and a line longer than
/// `STDERR_LINE_BYTES` is kept cut (see `read_capped_line`).
fn drain_stderr(server: String, stderr: ChildStderr, events: UnboundedSender<Msg>) {
    tokio::spawn(async move {
        let mut reader = BufReader::new(stderr);
        let mut buf = Vec::new();
        loop {
            match read_capped_line(&mut reader, &mut buf).await {
                Ok(true) => {
                    let line = String::from_utf8_lossy(&buf).trim_end_matches(['\r', '\n']).to_string();
                    let _ = events.send(Msg::Log { server: server.clone(), line });
                }
                Ok(false) | Err(_) => break,
            }
        }
    });
}

/// Lists the tools of an initialized client. On failure the client is closed before the error is returned.
async fn finish(
    mut service: RunningService<RoleClient, Handler>,
    http: bool,
) -> Result<(Live, Vec<Tool>), ConnectFailure> {
    match service.list_all_tools().await {
        Ok(tools) => {
            let peer = service.peer().clone();
            Ok((Live { service, peer }, tools))
        }
        Err(error) => {
            let auth = error.is_authorization_required() || error.auth_challenge().is_some();
            let failure = classify(error.to_string(), auth, http);
            let _ = service.close_with_timeout(CLOSE_TIMEOUT).await;
            Err(match failure {
                ConnectFailure::Failed(text) => ConnectFailure::Failed(format!("tools/list failed: {text}")),
                auth_failure => auth_failure,
            })
        }
    }
}

/// The failure of a handshake. The server wants credentials when rmcp reports an authorization challenge, or when an
/// HTTP server answered 401 or 403. Otherwise the failure is its text.
fn init_failure(error: &ClientInitializeError, http: bool) -> ConnectFailure {
    let auth = error.is_authorization_required() || error.auth_challenge().is_some();
    classify(error.to_string(), auth, http)
}

fn classify(text: String, auth_error: bool, http: bool) -> ConnectFailure {
    let http_auth = http && (text.contains("HTTP 401") || text.contains("HTTP 403"));
    if auth_error || http_auth {
        ConnectFailure::NeedsAuth(AUTH_MESSAGE.to_string())
    } else {
        ConnectFailure::Failed(text)
    }
}

/// The Streamable HTTP client config for `url` and its resolved headers. A `Authorization: Bearer <token>` header is
/// sent through rmcp's bearer auth (the token without the prefix). Every other header is sent as it is.
pub fn http_config(url: &str, headers: &[(String, String)]) -> Result<StreamableHttpClientTransportConfig, String> {
    let mut config = StreamableHttpClientTransportConfig::with_uri(url);
    let mut custom: HashMap<HeaderName, HeaderValue> = HashMap::new();
    for (name, value) in headers {
        if let Some(token) = value.strip_prefix("Bearer ").filter(|_| name.eq_ignore_ascii_case("authorization")) {
            config = config.auth_header(token);
            continue;
        }
        let header_name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| format!("header {name} is not a valid HTTP header name"))?;
        let header_value = HeaderValue::from_str(value)
            .map_err(|_| format!("the value of header {name} is not a valid HTTP header value"))?;
        custom.insert(header_name, header_value);
    }
    Ok(config.custom_headers(custom))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bearer_authorization_header_becomes_the_auth_token() {
        let headers = vec![("Authorization".to_string(), "Bearer tok-123".to_string())];
        let config = http_config("https://api.example.test/mcp", &headers).unwrap();
        assert_eq!(config.uri.as_ref(), "https://api.example.test/mcp");
        assert_eq!(config.auth_header.as_deref(), Some("tok-123"));
        assert!(config.custom_headers.is_empty());
    }

    #[test]
    fn other_headers_are_sent_as_custom_headers() {
        let headers = vec![
            ("CONTEXT7_API_KEY".to_string(), "k-1".to_string()),
            ("Authorization".to_string(), "Basic abc".to_string()),
        ];
        let config = http_config("http://localhost:9/mcp", &headers).unwrap();
        assert_eq!(config.auth_header, None);
        assert_eq!(config.custom_headers.get(&HeaderName::from_static("context7_api_key")).unwrap(), "k-1");
        assert_eq!(config.custom_headers.get(&HeaderName::from_static("authorization")).unwrap(), "Basic abc");
    }

    #[test]
    fn no_headers_means_no_auth_and_no_custom_headers() {
        let config = http_config("https://example.test/mcp", &[]).unwrap();
        assert_eq!(config.auth_header, None);
        assert!(config.custom_headers.is_empty());
    }

    #[test]
    fn an_invalid_header_name_or_value_is_an_error_that_names_only_the_header() {
        let bad_name = vec![("bad header".to_string(), "v".to_string())];
        let err = http_config("https://x.test/mcp", &bad_name).unwrap_err();
        assert_eq!(err, "header bad header is not a valid HTTP header name");

        let bad_value = vec![("X-Key".to_string(), "line\nbreak-secret".to_string())];
        let err = http_config("https://x.test/mcp", &bad_value).unwrap_err();
        assert!(err.contains("X-Key") && !err.contains("line"), "{err}");
    }

    #[tokio::test]
    async fn an_over_long_stderr_line_is_kept_cut_and_the_next_line_is_read() {
        let mut data = vec![b'a'; 1_100_000];
        data.extend_from_slice(b"\nafter the long line\n");
        let mut reader = BufReader::new(data.as_slice());
        let mut buf = Vec::new();
        assert!(read_capped_line(&mut reader, &mut buf).await.unwrap());
        assert_eq!(buf.len() as u64, STDERR_LINE_BYTES);
        assert!(buf.iter().all(|&b| b == b'a'));
        assert!(read_capped_line(&mut reader, &mut buf).await.unwrap());
        assert_eq!(buf, b"after the long line\n");
        assert!(!read_capped_line(&mut reader, &mut buf).await.unwrap(), "end of stream");
    }

    #[tokio::test]
    async fn a_stderr_line_within_the_cap_is_kept_whole() {
        let data = b"short\r\nlast line with no newline";
        let mut reader = BufReader::new(&data[..]);
        let mut buf = Vec::new();
        assert!(read_capped_line(&mut reader, &mut buf).await.unwrap());
        assert_eq!(buf, b"short\r\n");
        assert!(read_capped_line(&mut reader, &mut buf).await.unwrap());
        assert_eq!(buf, b"last line with no newline");
        assert!(!read_capped_line(&mut reader, &mut buf).await.unwrap());
    }
}
