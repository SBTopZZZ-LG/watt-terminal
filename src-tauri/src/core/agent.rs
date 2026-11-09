//! The agent: turns one user message into tool calls and events, and gates the tools that act.
//!
//! SECURITY: command output and screen contents are untrusted input to a model. Text printed in the
//! terminal (by a program, a file, or a web page) can try to steer a real LLM (prompt injection), and both
//! can contain secrets. So `run_command` asks for approval every time by default, `send_keys` always asks,
//! and auto-approve starts off. Auto-approve only skips the approval step; it never lets a command run
//! while the shell is busy.

use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};

use crate::core::session::Session;
use crate::core::tools::AgentTools;

pub const APPROVAL_TIMEOUT: Duration = Duration::from_secs(120);

/// Pending approvals, keyed by call id. A request that is not answered in time counts as a denial.
pub struct ApprovalGate {
    pending: Mutex<HashMap<String, Sender<bool>>>,
    auto: AtomicBool,
    timeout: Duration,
}

impl Default for ApprovalGate {
    fn default() -> Self {
        Self::new()
    }
}

impl ApprovalGate {
    pub fn new() -> Self {
        Self::with_timeout(APPROVAL_TIMEOUT)
    }

    pub fn with_timeout(timeout: Duration) -> Self {
        ApprovalGate { pending: Mutex::new(HashMap::new()), auto: AtomicBool::new(false), timeout }
    }

    pub fn set_auto_approve(&self, on: bool) {
        self.auto.store(on, Ordering::SeqCst);
    }

    pub fn auto_approve(&self) -> bool {
        self.auto.load(Ordering::SeqCst)
    }

    /// Registers `call_id`, runs `announce` so the UI can show the request, then waits. Only an explicit
    /// approval returns true. Denial, cancel and timeout return false.
    pub fn ask(&self, call_id: &str, announce: impl FnOnce()) -> bool {
        let (tx, rx) = mpsc::channel();
        self.lock().insert(call_id.to_string(), tx);
        announce();
        let answer = rx.recv_timeout(self.timeout).unwrap_or(false);
        self.lock().remove(call_id);
        answer
    }

    /// Answers a pending request. Returns false if `call_id` is not pending (already answered or timed out).
    pub fn decide(&self, call_id: &str, approve: bool) -> bool {
        let tx = self.lock().remove(call_id);
        tx.is_some_and(|tx| tx.send(approve).is_ok())
    }

    /// Denies every pending request. Called when the user presses Ctrl-C.
    pub fn cancel_all(&self) {
        for (_, tx) in self.lock().drain() {
            let _ = tx.send(false);
        }
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, Sender<bool>>> {
        self.pending.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Sends user keystrokes to the PTY. Ctrl-C (0x03) also denies every pending approval.
pub fn user_input(session: &Session, approvals: &ApprovalGate, text: &str) -> io::Result<()> {
    if text.contains('\u{3}') {
        approvals.cancel_all();
    }
    session.write_user(text.as_bytes())
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    Text { text: String },
    ToolCall { call_id: String, name: String, args: Value },
    ToolResult { call_id: String, name: String, ok: bool, result: Value },
    ApprovalRequest { call_id: String, text: String },
    Done,
}

static NEXT_CALL: AtomicU64 = AtomicU64::new(1);

/// Tools that never change the shell. `read_tool` accepts only these.
const READ_TOOLS: [&str; 4] = ["list_commands", "get_command", "get_screen", "get_cwd"];

/// What an agent may use during one turn: the approval gate and a way to report events. The tools are
/// private: the only route to them is `call_tool` (approval-gated) or `read_tool` (read-only names).
pub struct AgentCtx<'a> {
    tools: &'a dyn AgentTools,
    approvals: &'a ApprovalGate,
    emit: &'a dyn Fn(AgentEvent),
}

impl<'a> AgentCtx<'a> {
    pub fn new(tools: &'a dyn AgentTools, approvals: &'a ApprovalGate, emit: &'a dyn Fn(AgentEvent)) -> Self {
        AgentCtx { tools, approvals, emit }
    }

    pub fn emit(&self, ev: AgentEvent) {
        (self.emit)(ev);
    }

    pub fn say(&self, text: impl Into<String>) {
        self.emit(AgentEvent::Text { text: text.into() });
    }

    /// Calls a read-only tool (`list_commands`, `get_command`, `get_screen`, `get_cwd`) and reports the call
    /// and its result. Never asks for approval. Any other name is refused without running.
    pub fn read_tool(&self, name: &str, args: Value) -> Result<Value, String> {
        if !READ_TOOLS.contains(&name) {
            return Err(format!("`{name}` is not a read-only tool; it must go through call_tool"));
        }
        self.run(name, args, false)
    }

    /// Calls a tool and reports the call and its result. `run_command` and `send_keys` wait for approval
    /// first (`run_command` skips it when auto-approve is on). A denied call returns an error and does nothing.
    pub fn call_tool(&self, name: &str, args: Value) -> Result<Value, String> {
        self.run(name, args, true)
    }

    fn run(&self, name: &str, args: Value, gated: bool) -> Result<Value, String> {
        let call_id = format!("call-{}", NEXT_CALL.fetch_add(1, Ordering::Relaxed));
        self.emit(AgentEvent::ToolCall { call_id: call_id.clone(), name: name.to_string(), args: args.clone() });
        let needs_approval = gated
            && match name {
                "send_keys" => true,
                "run_command" => !self.approvals.auto_approve(),
                _ => false,
            };
        let result = if !needs_approval {
            self.tools.call(name, &args)
        } else {
            let text = args.get("command").and_then(Value::as_str).map_or_else(|| args.to_string(), str::to_string);
            let approved = self.approvals.ask(&call_id, || {
                self.emit(AgentEvent::ApprovalRequest { call_id: call_id.clone(), text: text.clone() })
            });
            if approved {
                self.tools.call(name, &args)
            } else {
                Err("not approved (denied, cancelled, or timed out)".to_string())
            }
        };
        let (ok, value) = match &result {
            Ok(v) => (true, v.clone()),
            Err(e) => (false, json!({"error": e})),
        };
        self.emit(AgentEvent::ToolResult { call_id, name: name.to_string(), ok, result: value });
        result
    }
}

pub trait Agent {
    /// Handles one user message. Replies and tool activity go out through `ctx`.
    fn respond(&mut self, user_text: &str, ctx: &AgentCtx<'_>);
}

/// Runs one turn and always ends it with `Done`.
pub fn run_turn(agent: &mut dyn Agent, user_text: &str, ctx: &AgentCtx<'_>) {
    agent.respond(user_text, ctx);
    ctx.emit(AgentEvent::Done);
}

/// Deterministic keyword rules over the tools. No network, no model.
pub struct MockAgent;

const HELP: &str = "I can tell you the last command and how it ended (\"what was the last command\"), \
describe the screen (\"what do you see\"), tell you the directory (\"where am I\"), or run a command \
once you approve it (\"run `pwd`\").";

impl Agent for MockAgent {
    fn respond(&mut self, user_text: &str, ctx: &AgentCtx<'_>) {
        let text = user_text.trim();
        // ASCII lowercasing keeps byte offsets equal to those in `text`.
        let lower = text.to_ascii_lowercase();
        if let Some(cmd) = run_request(text, &lower) {
            return run_and_report(ctx, &cmd);
        }
        if ["last command", "what happened", "why did it fail"].iter().any(|k| lower.contains(k)) {
            return last_command(ctx);
        }
        if lower.contains("screen") || lower.contains("what do you see") {
            return screen(ctx);
        }
        if lower.contains("where am i") || lower.contains("cwd") {
            return where_am_i(ctx);
        }
        ctx.say(HELP);
    }
}

/// "please run `cmd`" anywhere in the message, or "run cmd" at the start.
fn run_request(text: &str, lower: &str) -> Option<String> {
    const PLEASE: &str = "please run `";
    if let Some(start) = lower.find(PLEASE) {
        let rest = &text[start + PLEASE.len()..];
        return Some(rest.split('`').next().unwrap_or_default().trim().to_string());
    }
    if lower.starts_with("run ") {
        return Some(text[4..].trim().trim_matches('`').trim().to_string());
    }
    None
}

fn run_and_report(ctx: &AgentCtx<'_>, cmd: &str) {
    if cmd.is_empty() {
        ctx.say("Tell me what to run, for example: run `pwd`.");
        return;
    }
    match ctx.call_tool("run_command", json!({"command": cmd})) {
        Ok(v) if v["outcome"] == "finished" => {
            let r = &v["record"];
            ctx.say(format!(
                "Ran `{cmd}`: exit code {}. Output: {}.",
                code_text(&r["exit_code"]),
                output_text(r["output"].as_str().unwrap_or_default())
            ));
        }
        Ok(_) => ctx.say(format!(
            "`{cmd}` is still running after the 30 s wait. It keeps running in the terminal; ask for the last command once it finishes."
        )),
        Err(e) => ctx.say(format!("I did not run `{cmd}`: {e}.")),
    }
}

fn last_command(ctx: &AgentCtx<'_>) {
    let last = ctx
        .read_tool("list_commands", json!({"n": 1}))
        .ok()
        .and_then(|v| v.as_array().and_then(|a| a.last().cloned()));
    let Some(id) = last.and_then(|c| c["id"].as_u64()) else {
        ctx.say("No command has finished in this session yet.");
        return;
    };
    let view = match ctx.read_tool("get_command", json!({"id": id, "limit": 2000})) {
        Ok(v) => v,
        Err(e) => return ctx.say(format!("I could not read the last command: {e}.")),
    };
    let observer_ok = ctx.read_tool("get_screen", json!({})).ok().is_some_and(|s| s["observer_status"] == "ok");
    let cwd = match &view["cwd"] {
        Value::Null => "unknown".to_string(),
        c => format!("{}:{}", c["host"].as_str().unwrap_or("?"), c["path"].as_str().unwrap_or("?")),
    };
    ctx.say(format!(
        "Last command: `{}` (source: {}). Exit code {}. Directory: {}. Output: {}. Observer status: {}.",
        view["command"].as_str().unwrap_or_default(),
        view["source"]["kind"].as_str().unwrap_or("unknown"),
        code_text(&view["exit_code"]),
        cwd,
        output_text(view["output"].as_str().unwrap_or_default()),
        if observer_ok { "ok" } else { "desynced, so the output may be incomplete" },
    ));
}

fn screen(ctx: &AgentCtx<'_>) {
    match ctx.read_tool("get_screen", json!({})) {
        Ok(v) => {
            let rows: Vec<&str> = v["rows"]
                .as_array()
                .map(|r| r.iter().filter_map(Value::as_str).filter(|r| !r.trim().is_empty()).collect())
                .unwrap_or_default();
            let alt = if v["alternate_screen"].as_bool() == Some(true) { " (alternate screen)" } else { "" };
            ctx.say(format!("The screen{alt} shows:\n{}", rows.join("\n")));
        }
        Err(e) => ctx.say(format!("I could not read the screen: {e}.")),
    }
}

fn where_am_i(ctx: &AgentCtx<'_>) {
    match ctx.read_tool("get_cwd", json!({})) {
        Ok(Value::Null) => ctx.say("I don't know the working directory yet."),
        Ok(c) => ctx.say(format!(
            "You are in {}:{}.",
            c["host"].as_str().unwrap_or("?"),
            c["path"].as_str().unwrap_or("?")
        )),
        Err(e) => ctx.say(format!("I could not read the directory: {e}.")),
    }
}

fn code_text(v: &Value) -> String {
    v.as_i64().map_or_else(|| "unknown".to_string(), |c| c.to_string())
}

fn output_text(out: &str) -> String {
    match out.lines().collect::<Vec<_>>().as_slice() {
        [] => "(none)".to_string(),
        [only] => format!("\"{only}\""),
        [first, .., last] => format!("first line \"{first}\", last line \"{last}\""),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::core::observer::{InputState, IntegrationStatus, ObserverStatus};
    use crate::core::record::{Cwd, RecordSummary};
    use crate::core::tools::{CommandView, RunOutcome, ScreenView};

    /// Records every tool call the agent manages to reach. Nothing here touches a shell.
    #[derive(Default)]
    struct Recorder(Mutex<Vec<String>>);

    impl Recorder {
        fn reached(&self) -> Vec<String> {
            self.0.lock().unwrap().clone()
        }
    }

    impl AgentTools for Recorder {
        fn list_commands(&self, _n: usize) -> Vec<RecordSummary> {
            Vec::new()
        }
        fn get_command(&self, _id: u64, _offset: usize, _limit: usize) -> Result<CommandView, String> {
            Err("not used".into())
        }
        fn get_screen(&self) -> ScreenView {
            ScreenView {
                rows: Vec::new(),
                cursor: (0, 0),
                alternate_screen: false,
                size: (24, 80),
                state: InputState::Prompt,
                observer_status: ObserverStatus::Ok,
                integration_status: IntegrationStatus::Live,
            }
        }
        fn get_cwd(&self) -> Option<Cwd> {
            None
        }
        fn run_command(&self, _command: &str, _timeout: Duration) -> Result<RunOutcome, String> {
            Err("not used".into())
        }
        fn send_keys(&self, _bytes: &[u8]) -> Result<(), String> {
            Err("not used".into())
        }
        fn call(&self, name: &str, _args: &Value) -> Result<Value, String> {
            self.0.lock().unwrap().push(name.to_string());
            Ok(json!({"ok": true}))
        }
    }

    #[test]
    fn read_tool_refuses_write_tools_without_reaching_the_backend() {
        let tools = Recorder::default();
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        let emit = |_: AgentEvent| {};
        let ctx = AgentCtx::new(&tools, &gate, &emit);
        assert!(ctx.read_tool("run_command", json!({"command": "pwd"})).is_err());
        assert!(ctx.read_tool("send_keys", json!({"bytes": [13]})).is_err());
        assert!(tools.reached().is_empty(), "a write tool reached the backend: {:?}", tools.reached());
        assert!(ctx.read_tool("get_screen", json!({})).is_ok());
        assert_eq!(tools.reached(), vec!["get_screen".to_string()]);
    }

    #[test]
    fn call_tool_never_reaches_the_backend_without_approval() {
        let tools = Recorder::default();
        // No one answers, so the request times out and counts as a denial.
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        let emit = |_: AgentEvent| {};
        let ctx = AgentCtx::new(&tools, &gate, &emit);
        assert!(ctx.call_tool("send_keys", json!({"bytes": [13]})).is_err());
        assert!(ctx.call_tool("run_command", json!({"command": "pwd"})).is_err());
        assert!(tools.reached().is_empty(), "an unapproved call reached the backend: {:?}", tools.reached());
    }

    #[test]
    fn call_tool_skips_approval_for_read_tools() {
        let tools = Recorder::default();
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        let emit = |_: AgentEvent| {};
        let ctx = AgentCtx::new(&tools, &gate, &emit);
        assert!(ctx.call_tool("get_cwd", json!({})).is_ok());
        assert_eq!(tools.reached(), vec!["get_cwd".to_string()]);
    }
}
