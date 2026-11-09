//! The agent's tools: plain Rust methods over a session, plus JSON schemas so a model can call them later.
//!
//! Read tools are always available. `run_command` is the only tool that writes to the shell, and it is
//! refused unless the session is at an idle prompt with shell integration live. `send_keys` is a stub.
//! Approval is not checked here; `agent::AgentCtx::call_tool` asks for it before calling these methods.

use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};

use crate::core::observer::{InputState, IntegrationStatus, ObserverStatus};
use crate::core::record::{Cwd, Record, RecordSummary, Source};
use crate::core::session::Session;

/// How long `run_command` waits for the command to finish before it returns `TimedOut`.
pub const DEFAULT_RUN_TIMEOUT: Duration = Duration::from_secs(30);
const RUN_POLL: Duration = Duration::from_millis(25);
const DEFAULT_LIST: usize = 10;
const DEFAULT_LIMIT: usize = 4096;
const MAX_LIMIT: usize = 64 << 10;

#[derive(Debug, Clone, Serialize)]
pub struct ScreenView {
    pub rows: Vec<String>,
    /// (row, col), zero-based.
    pub cursor: (u16, u16),
    pub alternate_screen: bool,
    /// (rows, cols)
    pub size: (u16, u16),
    pub state: InputState,
    pub observer_status: ObserverStatus,
    pub integration_status: IntegrationStatus,
}

/// One page of a command's rendered output. `offset` and `limit` are byte counts, moved to character boundaries.
#[derive(Debug, Clone, Serialize)]
pub struct CommandView {
    #[serde(flatten)]
    pub record: RecordSummary,
    pub cwd: Option<Cwd>,
    pub output: String,
    pub offset: usize,
    pub total_bytes: usize,
    /// True when output continues after this page.
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum RunOutcome {
    /// The command finished within the timeout.
    Finished { record: Record },
    /// The command had not finished when the timeout expired. It keeps running; the shell is not touched.
    TimedOut { state: InputState },
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolSchema {
    pub name: &'static str,
    pub description: &'static str,
    pub parameters: Value,
}

/// What the agent may do. Methods return plain data; `call` takes and returns JSON.
pub trait AgentTools {
    /// The last `n` finished commands, oldest first.
    fn list_commands(&self, n: usize) -> Vec<RecordSummary>;
    fn get_command(&self, id: u64, offset: usize, limit: usize) -> Result<CommandView, String>;
    fn get_screen(&self) -> ScreenView;
    fn get_cwd(&self) -> Option<Cwd>;
    fn run_command(&self, command: &str, timeout: Duration) -> Result<RunOutcome, String>;
    fn send_keys(&self, bytes: &[u8]) -> Result<(), String>;

    /// Calls a tool by schema name. Optional arguments take their defaults. Does not ask for approval.
    fn call(&self, name: &str, args: &Value) -> Result<Value, String> {
        match name {
            "list_commands" => to_json(self.list_commands(usize_arg(args, "n", DEFAULT_LIST)?)),
            "get_command" => {
                let id = u64_arg(args, "id")?;
                let offset = usize_arg(args, "offset", 0)?;
                let limit = usize_arg(args, "limit", DEFAULT_LIMIT)?.min(MAX_LIMIT);
                to_json(self.get_command(id, offset, limit)?)
            }
            "get_screen" => to_json(self.get_screen()),
            "get_cwd" => to_json(self.get_cwd()),
            "run_command" => {
                let command = str_arg(args, "command")?;
                to_json(self.run_command(command, DEFAULT_RUN_TIMEOUT)?)
            }
            "send_keys" => {
                let bytes: Vec<u8> = serde_json::from_value(args.get("bytes").cloned().unwrap_or(Value::Null))
                    .map_err(|_| "argument `bytes` must be an array of integers 0-255".to_string())?;
                self.send_keys(&bytes)?;
                Ok(json!({"ok": true}))
            }
            other => Err(format!("unknown tool `{other}`")),
        }
    }
}

/// The tools over one live session.
pub struct SessionTools<'a> {
    session: &'a Session,
}

impl<'a> SessionTools<'a> {
    pub fn new(session: &'a Session) -> Self {
        SessionTools { session }
    }
}

static NEXT_RUN: AtomicU64 = AtomicU64::new(1);

impl AgentTools for SessionTools<'_> {
    fn list_commands(&self, n: usize) -> Vec<RecordSummary> {
        let all = self.session.records();
        all[all.len().saturating_sub(n)..].to_vec()
    }

    fn get_command(&self, id: u64, offset: usize, limit: usize) -> Result<CommandView, String> {
        let r = self.session.get_record(id).ok_or_else(|| format!("no finished command with id {id}"))?;
        let total = r.output.len();
        let start = char_floor(&r.output, offset.min(total));
        let mut end = char_floor(&r.output, start.saturating_add(limit).min(total));
        if end == start && start < total {
            // A limit smaller than one character still makes progress, so paging cannot get stuck.
            end = start + r.output[start..].chars().next().map_or(0, char::len_utf8);
        }
        Ok(CommandView {
            record: RecordSummary::from(&r),
            cwd: r.cwd.clone(),
            output: r.output[start..end].to_string(),
            offset: start,
            total_bytes: total,
            truncated: end < total,
        })
    }

    fn get_screen(&self) -> ScreenView {
        let snap = self.session.screen_snapshot();
        ScreenView {
            rows: snap.rows,
            cursor: snap.cursor,
            alternate_screen: snap.alternate_screen,
            size: snap.size,
            state: self.session.state(),
            observer_status: self.session.observer_status(),
            integration_status: self.session.integration_status(),
        }
    }

    fn get_cwd(&self) -> Option<Cwd> {
        self.session.cwd()
    }

    fn run_command(&self, command: &str, timeout: Duration) -> Result<RunOutcome, String> {
        if command.trim().is_empty() {
            return Err("command is empty".into());
        }
        // A CR or ESC inside the text would submit or escape early, so only plain newlines and tabs are allowed.
        if let Some(c) = command.chars().find(|c| c.is_control() && *c != '\n' && *c != '\t') {
            return Err(format!("command contains the control character {c:?}, which cannot be sent"));
        }
        if self.session.integration_status() != IntegrationStatus::Live {
            return Err("shell integration is not live, so commands cannot be run".into());
        }
        let state = self.session.state();
        if state != InputState::Prompt {
            return Err(format!("cannot run a command now: the session is {state:?}, not at an idle prompt"));
        }
        let call_id = format!("run-{}", NEXT_RUN.fetch_add(1, Ordering::Relaxed));
        // submit() re-checks the prompt atomically, so a racing user submit wins or loses cleanly.
        self.session
            .submit(Source::Agent { call_id: call_id.clone() }, command)
            .map_err(|e| e.to_string())?;

        let deadline = Instant::now() + timeout;
        loop {
            let finished = self
                .session
                .records()
                .into_iter()
                .find(|r| matches!(&r.source, Source::Agent { call_id: c } if *c == call_id))
                .map(|r| r.id);
            if let Some(id) = finished {
                let record = self.session.get_record(id).ok_or("the finished command disappeared")?;
                return Ok(RunOutcome::Finished { record });
            }
            if Instant::now() >= deadline {
                return Ok(RunOutcome::TimedOut { state: self.session.state() });
            }
            thread::sleep(RUN_POLL);
        }
    }

    fn send_keys(&self, _bytes: &[u8]) -> Result<(), String> {
        Err("send_keys is disabled in slice 1".into())
    }
}

/// The JSON schemas of every tool, in the form a model's tool-use API expects.
pub fn schemas() -> Vec<ToolSchema> {
    vec![
        ToolSchema {
            name: "list_commands",
            description: "List the most recent finished commands in the session, oldest first.",
            parameters: json!({
                "type": "object",
                "properties": {"n": {"type": "integer", "minimum": 0, "default": DEFAULT_LIST}},
            }),
        },
        ToolSchema {
            name: "get_command",
            description: "Get one finished command: its command line, exit code, cwd, and a page of its rendered output. Use offset and limit to page; truncated is true when output continues.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": {"type": "integer", "minimum": 1},
                    "offset": {"type": "integer", "minimum": 0, "default": 0},
                    "limit": {"type": "integer", "minimum": 1, "maximum": MAX_LIMIT, "default": DEFAULT_LIMIT},
                },
                "required": ["id"],
            }),
        },
        ToolSchema {
            name: "get_screen",
            description: "Get the visible screen: rows, cursor, whether the alternate screen is active, and the observer and shell-integration status.",
            parameters: json!({"type": "object", "properties": {}}),
        },
        ToolSchema {
            name: "get_cwd",
            description: "Get the shell's current host and directory, or null if not known yet.",
            parameters: json!({"type": "object", "properties": {}}),
        },
        ToolSchema {
            name: "run_command",
            description: "Run a command in the user's own shell session. Only works at an idle prompt with shell integration live. Waits up to 30 s for the command to finish. The user must approve each call unless auto-approve is on.",
            parameters: json!({
                "type": "object",
                "properties": {"command": {"type": "string", "minLength": 1}},
                "required": ["command"],
            }),
        },
        ToolSchema {
            name: "send_keys",
            description: "Send raw bytes to the terminal. Disabled in this version; always returns an error.",
            parameters: json!({
                "type": "object",
                "properties": {"bytes": {"type": "array", "items": {"type": "integer", "minimum": 0, "maximum": 255}}},
                "required": ["bytes"],
            }),
        },
    ]
}

fn to_json<T: Serialize>(v: T) -> Result<Value, String> {
    serde_json::to_value(v).map_err(|e| e.to_string())
}

fn usize_arg(args: &Value, key: &str, default: usize) -> Result<usize, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(v) => v.as_u64().map(|n| n as usize).ok_or_else(|| format!("argument `{key}` must be a non-negative integer")),
    }
}

fn u64_arg(args: &Value, key: &str) -> Result<u64, String> {
    args.get(key).and_then(Value::as_u64).ok_or_else(|| format!("argument `{key}` must be a non-negative integer"))
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key).and_then(Value::as_str).ok_or_else(|| format!("argument `{key}` must be a string"))
}

/// Largest index <= `i` that is a character boundary of `s`.
fn char_floor(s: &str, mut i: usize) -> usize {
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}
