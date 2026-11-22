//! The handle the app holds on the MCP runtime. It starts the `watt-mcp` thread (a current_thread tokio runtime that owns
//! every client, see runtime.rs), and it is the thread-safe API: each call sends a `Msg` and waits for the reply. It also
//! implements `ExternalToolHost`, so the agent (on its own std thread) can list, gate and call the tools.
//!
//! Blocking rules: a call to the runtime waits only for the runtime's own work, which never waits for a server. A tool
//! call waits for its reply and checks the turn's cancel every `CALL_POLL`.

use std::collections::BTreeMap;
use std::sync::mpsc::{self, RecvTimeoutError, Sender as StdSender};
use std::sync::{Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;

use crate::core::external_tools::{ExternalApproval, ExternalDisplay, ExternalToolHost, ExternalToolSchema};
use crate::core::llm::TurnCancel;

use super::config::McpSettings;
use super::runtime::{Msg, Runtime, SHUTDOWN_WAIT};
use super::secrets::SecretValues;

/// How often a waiting tool call checks the turn's cancel.
const CALL_POLL: Duration = Duration::from_millis(100);
const THREAD_NAME: &str = "watt-mcp";

/// The state of one server, for the UI and the logs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerState {
    Disabled,
    Connecting,
    Connected { tools: usize },
    Failed { error: String },
    NeedsAuth { message: String },
}

/// One server's state and its tool names (the server's own names).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerStatus {
    pub name: String,
    pub state: ServerState,
    pub tools: Vec<String>,
}

/// Where a model-facing tool name points, and how it is approved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolLookup {
    pub server: String,
    /// The server's own name for the tool.
    pub original: String,
    pub approval: ExternalApproval,
}

/// Owns the runtime thread. Dropping it shuts the runtime down (bounded, see `shutdown`).
pub struct McpManager {
    tx: UnboundedSender<Msg>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl McpManager {
    /// Starts the runtime thread. It starts with no servers: call `apply` to configure them.
    pub fn start() -> Result<McpManager, String> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| format!("could not start the MCP runtime: {e}"))?;
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let events = tx.clone();
        let thread = std::thread::Builder::new()
            .name(THREAD_NAME.to_string())
            .spawn(move || runtime.block_on(Runtime::new(events).run(rx)))
            .map_err(|e| format!("could not start the MCP thread: {e}"))?;
        Ok(McpManager { tx, thread: Mutex::new(Some(thread)) })
    }

    /// Makes the running servers match `config` (see runtime.rs `apply`). Returns once the runtime has done it; the
    /// connects continue in the background, so a server shows `Connecting` until it finishes.
    pub fn apply(&self, config: &McpSettings, secrets: &BTreeMap<String, SecretValues>) -> Result<(), String> {
        self.request(|reply| Msg::Apply { config: config.clone(), secrets: secrets.clone(), reply }).ok_or_else(unavailable)
    }

    /// Stops and starts one server now. An error when the server is unknown, disabled, or its settings cannot be resolved.
    pub fn restart(&self, server: &str) -> Result<(), String> {
        self.request(|reply| Msg::Restart { server: server.to_string(), reply }).unwrap_or_else(|| Err(unavailable()))
    }

    pub fn status(&self) -> Vec<ServerStatus> {
        self.request(|reply| Msg::Status { reply }).unwrap_or_default()
    }

    /// The last log lines of a server (oldest first), or `None` when the server is not configured.
    pub fn logs(&self, server: &str) -> Option<Vec<String>> {
        self.request(|reply| Msg::Logs { server: server.to_string(), reply }).flatten()
    }

    /// Closes every client and ends the thread. Waits at most `SHUTDOWN_WAIT` (plus a margin) for the runtime; if it does
    /// not answer in time, the thread is left to finish on its own rather than blocking the caller. Safe to call twice.
    pub fn shutdown(&self) {
        let thread = self.thread.lock().unwrap_or_else(PoisonError::into_inner).take();
        let Some(thread) = thread else { return };
        let (reply, answered) = mpsc::channel();
        if self.tx.send(Msg::Shutdown { reply }).is_err() {
            return;
        }
        if answered.recv_timeout(SHUTDOWN_WAIT + Duration::from_secs(5)).is_ok() {
            let _ = thread.join();
        }
    }

    /// Sends a request built from a reply channel and waits for the answer. `None` when the runtime is gone.
    fn request<T>(&self, build: impl FnOnce(StdSender<T>) -> Msg) -> Option<T> {
        let (reply, answer) = mpsc::channel();
        self.tx.send(build(reply)).ok()?;
        answer.recv().ok()
    }

    fn lookup(&self, tool: &str) -> Option<ToolLookup> {
        self.request(|reply| Msg::Lookup { tool: tool.to_string(), reply }).flatten()
    }
}

impl Drop for McpManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl ExternalToolHost for McpManager {
    fn schemas(&self) -> Vec<ExternalToolSchema> {
        self.request(|reply| Msg::Schemas { reply }).unwrap_or_default()
    }

    fn owns(&self, name: &str) -> bool {
        self.lookup(name).is_some()
    }

    fn approval(&self, name: &str) -> ExternalApproval {
        self.lookup(name).map_or(ExternalApproval::Never, |found| found.approval)
    }

    fn display(&self, name: &str, _args: &Value) -> ExternalDisplay {
        match self.lookup(name) {
            Some(found) => ExternalDisplay { server: found.server, tool: found.original },
            None => ExternalDisplay { server: String::new(), tool: name.to_string() },
        }
    }

    fn call(&self, name: &str, args: Value, cancel: &TurnCancel) -> Result<String, String> {
        let (reply, answer) = mpsc::channel();
        // Held until this function returns: dropping it tells the runtime the caller has gone.
        let (cancel_tx, cancel_rx) = oneshot::channel();
        if self.tx.send(Msg::Call { tool: name.to_string(), args, cancel: cancel_rx, reply }).is_err() {
            return Err(unavailable());
        }
        loop {
            match answer.recv_timeout(CALL_POLL) {
                Ok(outcome) => return outcome,
                Err(RecvTimeoutError::Timeout) => {
                    if cancel.is_cancelled() {
                        let _ = cancel_tx.send(());
                        return Err("cancelled".to_string());
                    }
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err("the MCP runtime stopped during the call".to_string());
                }
            }
        }
    }
}

fn unavailable() -> String {
    "the MCP runtime is not running".to_string()
}
