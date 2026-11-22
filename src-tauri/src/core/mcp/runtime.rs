//! The MCP runtime: the state of every configured server and the work done on it. It runs on the `watt-mcp` thread, inside
//! a current_thread tokio runtime, and it is the only place that owns a live client. Everything that touches a client
//! (connect, call, close) happens here, so a client is never dropped outside a runtime. Other threads talk to it through
//! `Msg` (see manager.rs for the calls and the blocking replies).
//!
//! Rules kept here: an unchanged running server is not restarted by `apply`; a call is never retried (its tool may have
//! side effects); a lost connection marks the server failed and, after a short delay, rebuilds the client at most once
//! per `RECONNECT_COOLDOWN`; a stopped server keeps its last tool names, so a call to it is refused with the reason.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::sync::mpsc::Sender as StdSender;
use std::time::{Duration, Instant};

use futures_util::future::{Either, select};
use rmcp::model::{CallToolRequest, CallToolRequestParams, CallToolResult, ClientRequest, JsonObject, ServerResult, Tool};
use rmcp::service::{PeerRequestOptions, ServiceError};
use rmcp::{Peer, RoleClient};
use serde_json::Value;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::core::chat::redact::Redactor;
use crate::core::external_tools::{ExternalApproval, ExternalToolSchema};

use super::approval;
use super::client::{self, CLOSE_TIMEOUT, ConnectFailure, Live};
use super::config::{McpLevel, McpServerConfig, McpSettings};
use super::expand::{ResolvedTransport, resolve_transport};
use super::logs::{LogRing, scrub};
use super::manager::{ServerState, ServerStatus, ToolLookup};
use super::names::NameTable;
use super::result::{cap_result, is_connection_lost, map_result, service_error_text};
use super::secrets::SecretValues;
use super::tools::{self, ToolInfo};

/// How long shutdown waits for the closes to finish. Each close is itself bounded by `CLOSE_TIMEOUT`.
pub const SHUTDOWN_WAIT: Duration = Duration::from_secs(5);
/// How often a connected server's transport is checked for having closed (a process that exited).
const WATCH_INTERVAL: Duration = Duration::from_millis(500);
/// The delay before the one automatic rebuild after a lost connection.
const RECONNECT_DELAY: Duration = Duration::from_secs(1);
/// The least time between two connect attempts that are not explicit (restart, apply).
pub const RECONNECT_COOLDOWN: Duration = Duration::from_secs(30);
/// Offered tools above this count are still offered, with a warning in the log.
pub const TOOL_WARN_LIMIT: usize = 64;

/// Everything the runtime thread receives: requests from the manager (with a reply channel), and events from its own
/// tasks (connects, watchers, stderr readers, call tasks).
pub enum Msg {
    Apply { config: McpSettings, secrets: BTreeMap<String, SecretValues>, reply: StdSender<()> },
    Restart { server: String, reply: StdSender<Result<(), String>> },
    Status { reply: StdSender<Vec<ServerStatus>> },
    Logs { server: String, reply: StdSender<Option<Vec<String>>> },
    Schemas { reply: StdSender<Vec<ExternalToolSchema>> },
    Lookup { tool: String, reply: StdSender<Option<ToolLookup>> },
    /// `cancel` resolves when the caller gives up on the call (or drops its sender).
    Call { tool: String, args: Value, cancel: oneshot::Receiver<()>, reply: StdSender<Result<String, String>> },
    Shutdown { reply: StdSender<()> },
    Connected { server: String, attempt: u64, result: Result<(Live, Vec<Tool>), ConnectFailure> },
    Lost { server: String, attempt: u64, reason: &'static str },
    Log { server: String, line: String },
    ToolListChanged { server: String, attempt: u64 },
    ToolsRefreshed { server: String, attempt: u64, result: Result<Vec<Tool>, String> },
    Reconnect { server: String },
}

/// The connection state of one server. `Connected` is the only state with a live client.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Status {
    Disabled,
    Connecting,
    Connected,
    Failed(String),
    NeedsAuth(String),
}

/// One configured server, as the runtime keeps it.
struct Server {
    config: McpServerConfig,
    /// The secret values of this server, as they were last applied (used again on restart).
    values: SecretValues,
    /// The transport of the last start, with its values filled in. `apply` compares it to decide whether to restart.
    resolved: Option<ResolvedTransport>,
    status: Status,
    /// Bumped on every start and stop, so the events of an old connect or client are recognised and ignored.
    attempt: u64,
    last_attempt: Option<Instant>,
    live: Option<Live>,
    connect: Option<JoinHandle<()>>,
    /// The last tool list. Kept across a stop, so a call is refused with the reason and not as unknown.
    tools: Vec<ToolInfo>,
    names: NameTable,
    logs: LogRing,
}

impl Server {
    fn new(config: McpServerConfig) -> Server {
        Server {
            config,
            values: SecretValues::default(),
            resolved: None,
            status: Status::Disabled,
            attempt: 0,
            last_attempt: None,
            live: None,
            connect: None,
            tools: Vec::new(),
            names: NameTable::default(),
            logs: LogRing::default(),
        }
    }

    /// Replaces the tool list with `tools`, named for this server.
    fn set_tools(&mut self, server: &str, tools: &[Tool]) {
        self.tools = tools.iter().map(ToolInfo::from_mcp).collect();
        let originals: Vec<String> = self.tools.iter().map(|t| t.original.clone()).collect();
        self.names = NameTable::build(server, &originals);
    }

    fn cooldown_elapsed(&self) -> bool {
        self.last_attempt.is_none_or(|at| at.elapsed() >= RECONNECT_COOLDOWN)
    }
}

pub struct Runtime {
    servers: BTreeMap<String, Server>,
    /// Built from every secret value applied. Used on log lines, errors and results.
    redactor: Arc<Redactor>,
    events: UnboundedSender<Msg>,
    /// Closes and aborted connects. Awaited at shutdown, so every drop of a client happens on this runtime.
    reapers: Vec<JoinHandle<()>>,
    /// Tool call tasks. Aborted at shutdown.
    calls: Vec<JoinHandle<()>>,
    /// Whether the tool-count warning was already logged, so it is logged once per crossing.
    over_limit: bool,
}

impl Runtime {
    pub fn new(events: UnboundedSender<Msg>) -> Runtime {
        Runtime {
            servers: BTreeMap::new(),
            redactor: Arc::new(Redactor::new(Vec::new())),
            events,
            reapers: Vec::new(),
            calls: Vec::new(),
            over_limit: false,
        }
    }

    /// Serves messages until `Shutdown`, then closes every client and returns. Runs on the runtime thread.
    pub async fn run(mut self, mut rx: UnboundedReceiver<Msg>) {
        let mut shutdown_reply = None;
        while let Some(msg) = rx.recv().await {
            if let Msg::Shutdown { reply } = msg {
                shutdown_reply = Some(reply);
                break;
            }
            self.handle(msg);
        }
        self.shutdown(rx).await;
        if let Some(reply) = shutdown_reply {
            let _ = reply.send(());
        }
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Apply { config, secrets, reply } => {
                self.apply(config, secrets);
                let _ = reply.send(());
            }
            Msg::Restart { server, reply } => {
                let _ = reply.send(self.restart(&server));
            }
            Msg::Status { reply } => {
                let _ = reply.send(self.status());
            }
            Msg::Logs { server, reply } => {
                let _ = reply.send(self.servers.get(&server).map(|s| s.logs.lines()));
            }
            Msg::Schemas { reply } => {
                let _ = reply.send(self.schemas());
            }
            Msg::Lookup { tool, reply } => {
                let _ = reply.send(self.lookup(&tool));
            }
            Msg::Call { tool, args, cancel, reply } => self.call(tool, args, cancel, reply),
            Msg::Connected { server, attempt, result } => self.connected(&server, attempt, result),
            Msg::Lost { server, attempt, reason } => self.lost(&server, attempt, reason),
            Msg::Log { server, line } => {
                let redactor = self.redactor.clone();
                if let Some(s) = self.servers.get_mut(&server) {
                    s.logs.push(&line, &redactor);
                }
            }
            Msg::ToolListChanged { server, attempt } => self.refresh_tools(&server, attempt),
            Msg::ToolsRefreshed { server, attempt, result } => self.tools_refreshed(&server, attempt, result),
            Msg::Reconnect { server } => self.reconnect(&server),
            // `run` handles this one and stops the loop.
            Msg::Shutdown { .. } => {}
        }
    }

    /// Stops every server, then waits (bounded by `SHUTDOWN_WAIT`) for the closes. A connect that finished but was not yet
    /// handled is closed too. Whatever is still running after the wait is aborted and awaited.
    async fn shutdown(&mut self, mut rx: UnboundedReceiver<Msg>) {
        for call in self.calls.drain(..) {
            call.abort();
        }
        let names: Vec<String> = self.servers.keys().cloned().collect();
        for name in &names {
            self.stop(name);
        }
        rx.close();
        while let Ok(msg) = rx.try_recv() {
            if let Msg::Connected { result: Ok((live, _)), .. } = msg {
                self.close(live);
            }
        }
        let deadline = tokio::time::Instant::now() + SHUTDOWN_WAIT;
        for mut handle in self.reapers.drain(..) {
            if tokio::time::timeout_at(deadline, &mut handle).await.is_err() {
                handle.abort();
                let _ = handle.await;
            }
        }
    }

    /// Makes the servers match `config`. A server that is new, changed (its transport or its secret values) or not running
    /// is started. A running server whose transport is unchanged keeps its client; its approval and timeout are read from
    /// the new config on the next use. A removed or disabled server is stopped.
    fn apply(&mut self, config: McpSettings, secrets: BTreeMap<String, SecretValues>) {
        let all_secret_values: Vec<String> = secrets
            .values()
            .flat_map(|v| v.env_values.values().chain(v.header_values.values()))
            .cloned()
            .collect();
        self.redactor = Arc::new(Redactor::new(all_secret_values));

        let wanted: HashSet<&str> = config.servers.iter().map(|s| s.name.as_str()).collect();
        let removed: Vec<String> = self.servers.keys().filter(|name| !wanted.contains(name.as_str())).cloned().collect();
        for name in removed {
            self.stop(&name);
            self.servers.remove(&name);
        }

        for server in config.servers {
            let name = server.name.clone();
            let enabled = server.enabled;
            let entry = self.servers.entry(name.clone()).or_insert_with(|| Server::new(server.clone()));
            entry.config = server;
            entry.values = secrets.get(&name).cloned().unwrap_or_default();
            if !enabled {
                self.stop(&name);
                continue;
            }
            match self.resolve(&name) {
                Err(error) => {
                    self.stop(&name);
                    let error = self.scrubbed(&error);
                    if let Some(s) = self.servers.get_mut(&name) {
                        s.resolved = None;
                        s.status = Status::Failed(error);
                    }
                }
                Ok(resolved) => {
                    let server = &self.servers[&name];
                    let running = matches!(server.status, Status::Connecting | Status::Connected);
                    if running && server.resolved.as_ref() == Some(&resolved) {
                        continue;
                    }
                    self.stop(&name);
                    self.start(&name, resolved);
                }
            }
        }
    }

    /// Restarts one server now, whatever its state (an explicit request, so the cooldown does not apply).
    fn restart(&mut self, name: &str) -> Result<(), String> {
        let Some(server) = self.servers.get(name) else {
            return Err(format!("no MCP server named '{name}'"));
        };
        if !server.config.enabled {
            return Err(format!("MCP server '{name}' is disabled: turn it on first"));
        }
        self.stop(name);
        match self.resolve(name) {
            Ok(resolved) => {
                self.start(name, resolved);
                Ok(())
            }
            Err(error) => {
                let error = self.scrubbed(&error);
                if let Some(s) = self.servers.get_mut(name) {
                    s.status = Status::Failed(error.clone());
                }
                Err(error)
            }
        }
    }

    fn status(&self) -> Vec<ServerStatus> {
        self.servers
            .iter()
            .map(|(name, server)| ServerStatus {
                name: name.clone(),
                state: match &server.status {
                    Status::Disabled => ServerState::Disabled,
                    Status::Connecting => ServerState::Connecting,
                    Status::Connected => ServerState::Connected { tools: server.tools.len() },
                    Status::Failed(error) => ServerState::Failed { error: error.clone() },
                    Status::NeedsAuth(message) => ServerState::NeedsAuth { message: message.clone() },
                },
                tools: server.tools.iter().map(|t| t.original.clone()).collect(),
            })
            .collect()
    }

    /// The tools offered now: every tool of every connected server whose approval is not `Never`.
    fn schemas(&mut self) -> Vec<ExternalToolSchema> {
        let mut out = Vec::new();
        for (name, server) in &self.servers {
            if server.status != Status::Connected {
                continue;
            }
            for (offered, original) in server.names.entries() {
                let Some(tool) = server.tools.iter().find(|t| t.original == original) else {
                    continue;
                };
                if approval_of(server, original) != ExternalApproval::Never {
                    out.push(tools::offered(name, offered, tool));
                }
            }
        }
        let over = out.len() > TOOL_WARN_LIMIT;
        if over && !self.over_limit {
            eprintln!(
                "MCP: {} tools are offered to the model (more than {TOOL_WARN_LIMIT}); a long tool list costs context",
                out.len()
            );
        }
        self.over_limit = over;
        out
    }

    fn lookup(&self, tool: &str) -> Option<ToolLookup> {
        self.servers.iter().find_map(|(name, server)| {
            let original = server.names.original(tool)?;
            Some(ToolLookup {
                server: name.clone(),
                original: original.to_string(),
                approval: approval_of(server, original),
            })
        })
    }

    /// Starts one call on its own task, so a slow call does not hold up the other messages. The reply is sent when the
    /// call ends, or as "cancelled" when `cancel` resolves first.
    fn call(&mut self, tool: String, args: Value, cancel: oneshot::Receiver<()>, reply: StdSender<Result<String, String>>) {
        let Some((server_name, original)) = self.servers.iter().find_map(|(name, s)| {
            s.names.original(&tool).map(|original| (name.clone(), original.to_string()))
        }) else {
            let _ = reply.send(Err(format!("unknown tool `{tool}`")));
            return;
        };
        let args = match args {
            Value::Null => JsonObject::new(),
            Value::Object(map) => map,
            _ => {
                let _ = reply.send(Err("the tool arguments must be a JSON object".to_string()));
                return;
            }
        };
        let connected = self.servers.get(&server_name).and_then(|s| {
            let live = s.live.as_ref()?;
            Some((live.peer.clone(), s.config.timeout(), s.attempt))
        });
        let Some((peer, timeout, attempt)) = connected else {
            let error = self.not_connected(&server_name);
            let _ = reply.send(Err(error));
            return;
        };
        let task = run_call(CallJob {
            peer,
            server: server_name,
            original,
            args,
            timeout,
            attempt,
            cancel,
            redactor: self.redactor.clone(),
            events: self.events.clone(),
            reply,
        });
        self.calls.retain(|h| !h.is_finished());
        self.calls.push(tokio::spawn(task));
    }

    /// The refusal for a call to a server that has no client. A failed server is rebuilt here if its cooldown has passed.
    fn not_connected(&mut self, server: &str) -> String {
        let Some(s) = self.servers.get(server) else {
            return format!("MCP server '{server}' is not connected");
        };
        let state = match &s.status {
            Status::Disabled => "disabled".to_string(),
            Status::Connecting => "connecting".to_string(),
            Status::Connected => "connected".to_string(),
            Status::Failed(error) => format!("failed: {error}"),
            Status::NeedsAuth(message) => format!("needs authentication: {message}"),
        };
        let retry = matches!(s.status, Status::Failed(_)) && s.cooldown_elapsed() && s.config.enabled;
        if retry {
            let _ = self.try_start(server);
            return format!("MCP server '{server}' is not connected ({state}); reconnecting, try again shortly");
        }
        format!("MCP server '{server}' is not connected ({state})")
    }

    fn connected(&mut self, name: &str, attempt: u64, result: Result<(Live, Vec<Tool>), ConnectFailure>) {
        let current = self
            .servers
            .get(name)
            .is_some_and(|s| s.attempt == attempt && s.status == Status::Connecting);
        if !current {
            // Stopped, restarted or removed while connecting: the client is closed, nothing else changes.
            if let Ok((live, _)) = result {
                self.close(live);
            }
            return;
        }
        let events = self.events.clone();
        let redactor = self.redactor.clone();
        let Some(server) = self.servers.get_mut(name) else { return };
        server.connect = None;
        match result {
            Ok((live, tools)) => {
                server.set_tools(name, &tools);
                spawn_watch(events, name.to_string(), attempt, live.peer.clone());
                server.live = Some(live);
                server.status = Status::Connected;
            }
            Err(ConnectFailure::Failed(error)) => {
                server.status = Status::Failed(scrub(&error, &redactor));
            }
            Err(ConnectFailure::NeedsAuth(message)) => {
                server.status = Status::NeedsAuth(scrub(&message, &redactor));
            }
        }
    }

    /// The client's transport closed while it was connected: the server exited or the connection broke. The client is
    /// closed, the server is marked failed, and one rebuild is scheduled (it waits out the cooldown when that is not due).
    fn lost(&mut self, name: &str, attempt: u64, reason: &'static str) {
        let Some(server) = self.servers.get_mut(name) else { return };
        if server.attempt != attempt || server.status != Status::Connected {
            return;
        }
        let live = server.live.take();
        server.status = Status::Failed(reason.to_string());
        if let Some(live) = live {
            self.close(live);
        }
        let events = self.events.clone();
        let server = name.to_string();
        tokio::spawn(async move {
            tokio::time::sleep(RECONNECT_DELAY).await;
            let _ = events.send(Msg::Reconnect { server });
        });
    }

    fn reconnect(&mut self, name: &str) {
        let due = self
            .servers
            .get(name)
            .is_some_and(|s| matches!(s.status, Status::Failed(_)) && s.cooldown_elapsed() && s.config.enabled);
        if due {
            let _ = self.try_start(name);
        }
    }

    fn refresh_tools(&mut self, name: &str, attempt: u64) {
        let Some(server) = self.servers.get(name) else { return };
        if server.attempt != attempt || server.status != Status::Connected {
            return;
        }
        let Some(live) = server.live.as_ref() else { return };
        let peer = live.peer.clone();
        let events = self.events.clone();
        let server = name.to_string();
        tokio::spawn(async move {
            let result = peer.list_all_tools().await.map_err(|e| e.to_string());
            let _ = events.send(Msg::ToolsRefreshed { server, attempt, result });
        });
    }

    fn tools_refreshed(&mut self, name: &str, attempt: u64, result: Result<Vec<Tool>, String>) {
        let redactor = self.redactor.clone();
        let Some(server) = self.servers.get_mut(name) else { return };
        if server.attempt != attempt || server.status != Status::Connected {
            return;
        }
        match result {
            Ok(tools) => server.set_tools(name, &tools),
            Err(error) => {
                let line = format!("tools/list after a list change failed: {error}");
                server.logs.push(&line, &redactor);
            }
        }
    }

    /// Stops a server: aborts its connect, closes its client, and marks it disabled. The tool names are kept.
    fn stop(&mut self, name: &str) {
        let Some(server) = self.servers.get_mut(name) else { return };
        server.attempt += 1;
        let connect = server.connect.take();
        let live = server.live.take();
        server.status = Status::Disabled;
        if let Some(connect) = connect {
            connect.abort();
            self.reapers.push(connect);
        }
        if let Some(live) = live {
            self.close(live);
        }
    }

    /// Starts a connect for `name` with `resolved` as its transport.
    fn start(&mut self, name: &str, resolved: ResolvedTransport) {
        let events = self.events.clone();
        let Some(server) = self.servers.get_mut(name) else { return };
        server.attempt += 1;
        server.last_attempt = Some(Instant::now());
        server.status = Status::Connecting;
        server.resolved = Some(resolved.clone());
        let attempt = server.attempt;
        let owner = name.to_string();
        server.connect = Some(tokio::spawn(async move {
            let result = client::connect(&owner, attempt, resolved, events.clone()).await;
            let _ = events.send(Msg::Connected { server: owner, attempt, result });
        }));
    }

    /// Resolves the server's transport and starts it. A resolution error becomes the failed status.
    fn try_start(&mut self, name: &str) -> Result<(), String> {
        match self.resolve(name) {
            Ok(resolved) => {
                self.start(name, resolved);
                Ok(())
            }
            Err(error) => {
                let error = self.scrubbed(&error);
                if let Some(s) = self.servers.get_mut(name) {
                    s.status = Status::Failed(error.clone());
                }
                Err(error)
            }
        }
    }

    /// The transport of `name` with every reference expanded and every secret value filled in.
    fn resolve(&self, name: &str) -> Result<ResolvedTransport, String> {
        let server = self.servers.get(name).ok_or_else(|| format!("no MCP server named '{name}'"))?;
        resolve_transport(&server.config.transport, &server.values, &|key: &str| std::env::var(key).ok())
    }

    fn scrubbed(&self, text: &str) -> String {
        scrub(text, &self.redactor)
    }

    /// Closes a client on a reaper task (bounded), then drops it there.
    fn close(&mut self, live: Live) {
        let Live { mut service, .. } = live;
        self.reapers.retain(|h| !h.is_finished());
        self.reapers.push(tokio::spawn(async move {
            let _ = service.close_with_timeout(CLOSE_TIMEOUT).await;
            drop(service);
        }));
    }
}

/// The approval of one tool of `server`: its enabled switch, the server level and the tool's own override.
fn approval_of(server: &Server, original: &str) -> ExternalApproval {
    let override_level: Option<McpLevel> = server.config.tools.get(original).copied();
    approval::resolve(server.config.enabled, server.config.approval, override_level)
}

/// Watches a connected client for its transport closing (the process exited). Reports it once, then stops.
fn spawn_watch(events: UnboundedSender<Msg>, server: String, attempt: u64, peer: Peer<RoleClient>) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(WATCH_INTERVAL).await;
            if peer.is_transport_closed() {
                let _ = events.send(Msg::Lost { server, attempt, reason: "server exited" });
                break;
            }
        }
    });
}

/// What a call task needs. Built on the runtime thread, moved into the task.
struct CallJob {
    peer: Peer<RoleClient>,
    server: String,
    original: String,
    args: JsonObject,
    timeout: Duration,
    attempt: u64,
    cancel: oneshot::Receiver<()>,
    redactor: Arc<Redactor>,
    events: UnboundedSender<Msg>,
    reply: StdSender<Result<String, String>>,
}

/// Runs one call and sends its text back. A connection that broke during the call is reported as `Lost`. The text is
/// redacted and capped before it leaves the runtime. Dropping the call future (on cancel or timeout) makes rmcp send
/// `notifications/cancelled` to the server.
async fn run_call(job: CallJob) {
    let CallJob { peer, server, original, args, timeout, attempt, cancel, redactor, events, reply } = job;
    let call = call_tool(&peer, original, args, timeout);
    let cancelled = async {
        let _ = cancel.await;
    };
    let outcome = match select(Box::pin(call), Box::pin(cancelled)).await {
        Either::Left((Ok(result), _)) => map_result(&result),
        Either::Left((Err(error), _)) => {
            if is_connection_lost(&error) {
                let _ = events.send(Msg::Lost { server: server.clone(), attempt, reason: "connection lost" });
            }
            Err(service_error_text(&error, &server, timeout))
        }
        Either::Right(_) => Err("cancelled".to_string()),
    };
    let outcome = outcome
        .map(|text| cap_result(scrub(&text, &redactor)))
        .map_err(|text| cap_result(scrub(&text, &redactor)));
    let _ = reply.send(outcome);
}

/// One `tools/call` with the per-call timeout. rmcp sends the timeout's cancellation to the server.
async fn call_tool(
    peer: &Peer<RoleClient>,
    name: String,
    args: JsonObject,
    timeout: Duration,
) -> Result<CallToolResult, ServiceError> {
    let params = CallToolRequestParams::new(name).with_arguments(args);
    let request = ClientRequest::CallToolRequest(CallToolRequest::new(params));
    let handle = peer.send_cancellable_request(request, PeerRequestOptions::with_timeout(timeout)).await?;
    match handle.await_response().await? {
        ServerResult::CallToolResult(result) => Ok(result),
        _ => Err(ServiceError::UnexpectedResponse),
    }
}
