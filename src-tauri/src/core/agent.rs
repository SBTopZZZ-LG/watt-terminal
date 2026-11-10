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
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};

use crate::core::chat::todo::Todo;
use crate::core::llm::TurnCancel;
use crate::core::session::{Session, USER_TEXT_CODE};
use crate::core::tools::AgentTools;

pub const APPROVAL_TIMEOUT: Duration = Duration::from_secs(120);

/// Prefix of the error for a call the user did not approve: denied, cancelled, or timed out. Callers match on it to tell
/// a denial from a tool error. The text after it is for people.
pub const DENIED: &str = "not_approved";

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

/// Cancels the turn running on this session, if there is one, and denies every pending approval. Ctrl-C and the
/// cancel command both call it. Idempotent, and harmless when no turn is running. A cancelled `run_command` returns
/// at once; a command it already sent keeps running in the shell.
pub fn cancel_turn(current: &Mutex<Option<Arc<TurnCancel>>>, approvals: &ApprovalGate) {
    let token = current.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(token) = token {
        token.cancel();
    }
    approvals.cancel_all();
}

/// Runs a closure when dropped, including while a panic unwinds. Used so a turn cannot leave flags set.
pub struct OnDrop<F: FnOnce()>(Option<F>);

impl<F: FnOnce()> OnDrop<F> {
    pub fn new(f: F) -> Self {
        OnDrop(Some(f))
    }
}

impl<F: FnOnce()> Drop for OnDrop<F> {
    fn drop(&mut self) {
        if let Some(f) = self.0.take() {
            f();
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    Text { text: String },
    ToolCall { call_id: String, name: String, args: Value },
    /// `decision` says how the call was allowed or stopped: "read" (a read tool, or `todo_write`: no approval), "manual"
    /// (the user approved), "auto" (auto-approve skipped the card), "denied" (not approved: denied, cancelled or timed out),
    /// or "refused" (refused before it ran, without asking, or by the tool when auto-approve skipped the card).
    ToolResult { call_id: String, name: String, ok: bool, decision: &'static str, result: Value },
    /// A call waits for the user. `name` is the tool name, so the card can say what is being asked.
    ApprovalRequest { call_id: String, name: String, text: String },
    /// Streamed assistant text. `Text` is still used for whole messages.
    TextDelta { text: String },
    /// The model is working on a reply (true) or has produced its first output (false).
    Thinking { active: bool },
    /// The agent is in control of the shell: an approved `run_command` or `send_keys` is executing. Sent true before
    /// the call and false after it. A denied call sends nothing. A call the tool refuses (for example, the shell is
    /// busy) is bracketed too, but the bracket is immediate.
    AgentControl { active: bool },
    /// The conversation's todo list was replaced by a successful `todo_write`. `items` is the whole list.
    TodoUpdate { items: Vec<Todo> },
    Done,
}

static NEXT_CALL: AtomicU64 = AtomicU64::new(1);

fn new_call_id() -> String {
    format!("call-{}", NEXT_CALL.fetch_add(1, Ordering::Relaxed))
}

/// Tools that never change the shell. `read_tool` accepts only these. Must match `READ_NAMES` in core/chat/agent_loop.rs.
const READ_TOOLS: [&str; 5] = ["list_commands", "get_command", "get_screen", "get_cwd", "wait"];

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

    /// Calls a read-only tool (`list_commands`, `get_command`, `get_screen`, `get_cwd`, `wait`) and reports the call
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

    /// Runs a tool the agent loop handles itself (`todo_write`): no approval, no shell, no backend. It reports the call
    /// and its result like any tool, with the "read" decision, since no approval was asked.
    pub fn local_tool(
        &self,
        name: &str,
        args: &Value,
        run: impl FnOnce(&Value) -> Result<Value, String>,
    ) -> Result<Value, String> {
        let call_id = new_call_id();
        self.emit(AgentEvent::ToolCall { call_id: call_id.clone(), name: name.to_string(), args: args.clone() });
        let result = run(args);
        let (ok, value) = match &result {
            Ok(v) => (true, v.clone()),
            Err(e) => (false, json!({"error": e})),
        };
        self.emit(AgentEvent::ToolResult { call_id, name: name.to_string(), ok, decision: "read", result: value });
        result
    }

    fn run(&self, name: &str, args: Value, gated: bool) -> Result<Value, String> {
        let call_id = new_call_id();
        self.emit(AgentEvent::ToolCall { call_id: call_id.clone(), name: name.to_string(), args: args.clone() });
        let (decision, result) = self.gate_and_execute(&call_id, name, &args, gated);
        let (ok, value) = match &result {
            Ok(v) => (true, v.clone()),
            Err(e) => (false, json!({"error": e})),
        };
        self.emit(AgentEvent::ToolResult { call_id, name: name.to_string(), ok, decision, result: value });
        result
    }

    /// Applies the gate to one call and runs it if allowed. Returns the decision label with the result.
    fn gate_and_execute(
        &self,
        call_id: &str,
        name: &str,
        args: &Value,
        gated: bool,
    ) -> (&'static str, Result<Value, String>) {
        if !gated || READ_TOOLS.contains(&name) {
            return ("read", self.execute(name, args, false));
        }
        // A run_command the user's typed text would block is refused before the approval card: the user is not asked to
        // approve it. The tool checks again when it injects, in case the user types while the card waits.
        if name == "run_command"
            && let Err(e) = self.tools.run_command_precheck()
        {
            return ("refused", Err(e));
        }
        let needs_approval = match name {
            "send_keys" => true,
            "run_command" => !self.approvals.auto_approve(),
            _ => false,
        };
        if !needs_approval {
            // Auto-approve skipped the card. A tool that refuses here did not run and the user was never asked.
            let result = self.execute(name, args, true);
            return (if result.is_ok() { "auto" } else { "refused" }, result);
        }
        let text = args.get("command").and_then(Value::as_str).map_or_else(|| args.to_string(), str::to_string);
        let approved = self.approvals.ask(call_id, || {
            self.emit(AgentEvent::ApprovalRequest {
                call_id: call_id.to_string(),
                name: name.to_string(),
                text: text.clone(),
            })
        });
        if !approved {
            return ("denied", Err(format!("{DENIED}: the user did not approve this call (denied, cancelled, or timed out)")));
        }
        // The user approved, so the decision stays "manual" even if the tool then refuses.
        ("manual", self.execute(name, args, true))
    }

    /// Calls the tool. For the calls that act on the shell, the agent is in control from before the call until after it.
    fn execute(&self, name: &str, args: &Value, gated: bool) -> Result<Value, String> {
        let controls_shell = gated && matches!(name, "run_command" | "send_keys");
        if controls_shell {
            self.emit(AgentEvent::AgentControl { active: true });
        }
        let result = self.tools.call(name, args);
        if controls_shell {
            self.emit(AgentEvent::AgentControl { active: false });
        }
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
        Ok(v) => ctx.say(outcome_text(cmd, &v)),
        Err(e) if e.starts_with(USER_TEXT_CODE) => {
            ctx.say("I can't run that: you have text typed at the prompt. Clear it (Ctrl-C) and ask again.")
        }
        Err(e) if e.starts_with(DENIED) => {
            ctx.say(format!("I did not run `{cmd}`: it was not approved (denied, cancelled, or timed out)."))
        }
        Err(e) => ctx.say(format!("I did not run `{cmd}`: {e}.")),
    }
}

/// The chat text for a `run_command` result that came back (not an error). Only a shell in the Running state is
/// described as still running.
fn outcome_text(cmd: &str, v: &Value) -> String {
    match v["outcome"].as_str() {
        Some("finished") => {
            let r = &v["record"];
            format!(
                "Ran `{cmd}`: exit code {}. Output: {}.",
                code_text(&r["exit_code"]),
                output_text(r["output"].as_str().unwrap_or_default())
            )
        }
        Some("timed_out") if v["state"] == "running" => format!(
            "`{cmd}` is still running after the 30 s wait. It keeps running in the terminal; ask for the last command once it finishes."
        ),
        Some("timed_out") => format!(
            "`{cmd}` got no result within the 30 s wait, and the shell is not running it (shell state: {}). Ask for the last command to see whether it ran.",
            v["state"].as_str().unwrap_or("unknown")
        ),
        Some("not_run") if v["reason"] == "abandoned" => format!(
            "`{cmd}` did not run: the shell went back to the prompt without executing it (the input was cancelled or incomplete)."
        ),
        Some("not_run") if v["reason"] == "cancelled" => format!(
            "I stopped waiting for `{cmd}` because you cancelled it. If it had already been sent to the shell, it keeps running there; Ctrl-C in the terminal stops it."
        ),
        Some("not_run") => format!(
            "`{cmd}` did not run yet: the shell is waiting for the rest of a multi-line command (an unclosed quote or backslash). Press Ctrl-C in the terminal to cancel it, then ask again."
        ),
        _ => format!("I got an unexpected result for `{cmd}`."),
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
        c => c["path"].as_str().unwrap_or("?").to_string(),
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
        Ok(c) => ctx.say(format!("You are in {}.", c["path"].as_str().unwrap_or("?"))),
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
    use std::thread;

    use super::*;
    use crate::core::observer::{InputState, IntegrationStatus, ObserverStatus};
    use crate::core::record::{Cwd, RecordSummary};
    use crate::core::tools::{CommandView, RunOutcome, ScreenView, WaitView};

    /// Records every tool call the agent manages to reach. Nothing here touches a shell.
    #[derive(Default)]
    struct Recorder {
        reached: Mutex<Vec<String>>,
        /// What `run_command_precheck` returns as an error. None means the precheck passes.
        precheck: Option<String>,
        /// When set, every `call` fails with this error after it is recorded.
        call_error: Option<String>,
    }

    impl Recorder {
        fn reached(&self) -> Vec<String> {
            self.reached.lock().unwrap().clone()
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
        fn wait(&self, _seconds: u64, _ignore_updates: bool) -> Result<WaitView, String> {
            Err("not used".into())
        }
        fn send_keys(&self, _bytes: &[u8]) -> Result<(), String> {
            Err("not used".into())
        }
        fn run_command_precheck(&self) -> Result<(), String> {
            self.precheck.clone().map_or(Ok(()), Err)
        }
        fn call(&self, name: &str, _args: &Value) -> Result<Value, String> {
            self.reached.lock().unwrap().push(name.to_string());
            match &self.call_error {
                Some(e) => Err(e.clone()),
                None => Ok(json!({"ok": true})),
            }
        }
    }

    /// `(name, decision, ok)` of every ToolResult, in order.
    fn decisions(log: &Mutex<Vec<AgentEvent>>) -> Vec<(String, &'static str, bool)> {
        log.lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                AgentEvent::ToolResult { name, decision, ok, .. } => Some((name.clone(), *decision, *ok)),
                _ => None,
            })
            .collect()
    }

    fn named(name: &str, decision: &'static str, ok: bool) -> (String, &'static str, bool) {
        (name.to_string(), decision, ok)
    }

    #[test]
    fn serialized_events_use_the_wire_names() {
        let wire = |e: AgentEvent| serde_json::to_value(&e).unwrap();
        assert_eq!(
            wire(AgentEvent::ApprovalRequest { call_id: "c1".into(), name: "run_command".into(), text: "pwd".into() }),
            json!({"type": "approval_request", "call_id": "c1", "name": "run_command", "text": "pwd"})
        );
        assert_eq!(
            wire(AgentEvent::ToolResult {
                call_id: "c1".into(),
                name: "run_command".into(),
                ok: false,
                decision: "denied",
                result: json!({"error": "x"}),
            }),
            json!({"type": "tool_result", "call_id": "c1", "name": "run_command", "ok": false, "decision": "denied", "result": {"error": "x"}})
        );
        assert_eq!(wire(AgentEvent::TextDelta { text: "Hel".into() }), json!({"type": "text_delta", "text": "Hel"}));
        assert_eq!(wire(AgentEvent::Thinking { active: true }), json!({"type": "thinking", "active": true}));
        assert_eq!(wire(AgentEvent::Done), json!({"type": "done"}));
        assert_eq!(
            wire(AgentEvent::TodoUpdate {
                items: vec![Todo { content: "fix it".into(), status: crate::core::chat::todo::TodoStatus::InProgress }],
            }),
            json!({"type": "todo_update", "items": [{"content": "fix it", "status": "in_progress"}]})
        );
    }

    #[test]
    fn decision_is_read_for_read_tools() {
        let tools = Recorder::default();
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        let log = Mutex::new(Vec::new());
        let emit = |ev: AgentEvent| log.lock().unwrap().push(ev);
        let ctx = AgentCtx::new(&tools, &gate, &emit);
        assert!(ctx.read_tool("get_screen", json!({})).is_ok());
        assert!(ctx.call_tool("get_cwd", json!({})).is_ok());
        assert_eq!(decisions(&log), [named("get_screen", "read", true), named("get_cwd", "read", true)]);
    }

    /// `wait` is a read tool: it goes through `read_tool` with the "read" decision, and never shows an approval card.
    #[test]
    fn wait_is_a_read_tool_and_never_asks_for_approval() {
        let tools = Recorder::default();
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        let log = Mutex::new(Vec::new());
        let emit = |ev: AgentEvent| log.lock().unwrap().push(ev);
        let ctx = AgentCtx::new(&tools, &gate, &emit);
        assert!(ctx.read_tool("wait", json!({"seconds": 1})).is_ok());
        assert_eq!(decisions(&log), [named("wait", "read", true)]);
        assert_eq!(tools.reached(), ["wait"]);
        assert!(!log.lock().unwrap().iter().any(|e| matches!(e, AgentEvent::ApprovalRequest { .. })));
    }

    #[test]
    fn decision_is_manual_when_the_user_approves() {
        let tools = Recorder::default();
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        let log = Mutex::new(Vec::new());
        // The card's emit runs after the call is registered, so the answer is already waiting when `ask` reads it.
        let emit = |ev: AgentEvent| {
            if let AgentEvent::ApprovalRequest { call_id, name, .. } = &ev {
                assert_eq!(name, "run_command", "the card names the tool");
                assert!(gate.decide(call_id, true));
            }
            log.lock().unwrap().push(ev);
        };
        let ctx = AgentCtx::new(&tools, &gate, &emit);
        assert!(ctx.call_tool("run_command", json!({"command": "pwd"})).is_ok());
        assert_eq!(decisions(&log), [named("run_command", "manual", true)]);
        assert_eq!(tools.reached(), ["run_command"]);
    }

    #[test]
    fn decision_is_auto_when_auto_approve_skips_the_card() {
        let tools = Recorder::default();
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        gate.set_auto_approve(true);
        let log = Mutex::new(Vec::new());
        let emit = |ev: AgentEvent| log.lock().unwrap().push(ev);
        let ctx = AgentCtx::new(&tools, &gate, &emit);
        assert!(ctx.call_tool("run_command", json!({"command": "pwd"})).is_ok());
        assert_eq!(decisions(&log), [named("run_command", "auto", true)]);
        assert!(!log.lock().unwrap().iter().any(|e| matches!(e, AgentEvent::ApprovalRequest { .. })));
    }

    #[test]
    fn decision_is_denied_when_nobody_approves_and_the_error_says_so() {
        let tools = Recorder::default();
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        let log = Mutex::new(Vec::new());
        let emit = |ev: AgentEvent| log.lock().unwrap().push(ev);
        let ctx = AgentCtx::new(&tools, &gate, &emit);
        let err = ctx.call_tool("run_command", json!({"command": "pwd"})).expect_err("timed out");
        assert!(err.starts_with(DENIED), "{err}");
        assert_eq!(decisions(&log), [named("run_command", "denied", false)]);
        assert!(tools.reached().is_empty());
    }

    #[test]
    fn decision_is_refused_when_the_precheck_refuses_without_a_card() {
        let tools = Recorder { precheck: Some("prompt_has_user_text: typed".into()), ..Default::default() };
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        let log = Mutex::new(Vec::new());
        let emit = |ev: AgentEvent| log.lock().unwrap().push(ev);
        let ctx = AgentCtx::new(&tools, &gate, &emit);
        let err = ctx.call_tool("run_command", json!({"command": "pwd"})).expect_err("refused");
        assert!(!err.starts_with(DENIED), "a precheck refusal is not a denial: {err}");
        assert_eq!(decisions(&log), [named("run_command", "refused", false)]);
        assert!(!log.lock().unwrap().iter().any(|e| matches!(e, AgentEvent::ApprovalRequest { .. })));
    }

    #[test]
    fn decision_is_refused_when_auto_approved_tool_refuses_and_manual_when_approved_tool_refuses() {
        let log = Mutex::new(Vec::new());
        let emit = |ev: AgentEvent| log.lock().unwrap().push(ev);

        // Auto-approve skipped the card, then the tool refused: the user never saw it, so it is "refused".
        let tools = Recorder { call_error: Some("shell is busy".into()), ..Default::default() };
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        gate.set_auto_approve(true);
        let ctx = AgentCtx::new(&tools, &gate, &emit);
        let err = ctx.call_tool("run_command", json!({"command": "pwd"})).expect_err("tool error");
        assert!(!err.starts_with(DENIED), "{err}");
        assert_eq!(decisions(&log), [named("run_command", "refused", false)]);

        // The user approved, then the tool refused: the approval happened, so it stays "manual".
        log.lock().unwrap().clear();
        let tools = Recorder { call_error: Some("shell is busy".into()), ..Default::default() };
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        let emit = |ev: AgentEvent| {
            if let AgentEvent::ApprovalRequest { call_id, .. } = &ev {
                assert!(gate.decide(call_id, true));
            }
            log.lock().unwrap().push(ev);
        };
        let ctx = AgentCtx::new(&tools, &gate, &emit);
        assert!(ctx.call_tool("run_command", json!({"command": "pwd"})).is_err());
        assert_eq!(decisions(&log), [named("run_command", "manual", false)]);
    }

    #[test]
    fn cancel_turn_cancels_the_token_and_denies_pending_approvals() {
        let slot: Mutex<Option<Arc<TurnCancel>>> = Mutex::new(None);
        let gate = ApprovalGate::new();
        // No turn running: harmless, and repeatable.
        cancel_turn(&slot, &gate);
        cancel_turn(&slot, &gate);

        let token = Arc::new(TurnCancel::new());
        *slot.lock().unwrap() = Some(token.clone());
        let approved = thread::scope(|s| {
            let (tx, rx) = mpsc::channel();
            let gate_ref = &gate;
            // The waiter reports once its request is registered, so the cancel finds it pending.
            let waiter = s.spawn(move || gate_ref.ask("pending-call", move || tx.send(()).unwrap()));
            rx.recv().unwrap();
            cancel_turn(&slot, &gate);
            waiter.join().unwrap()
        });
        assert!(!approved, "a cancelled approval must count as a denial");
        assert!(token.is_cancelled());
        cancel_turn(&slot, &gate);
        assert!(token.is_cancelled());
        assert!(!gate.decide("pending-call", true), "the cancelled request is no longer pending");
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

    /// One label per event, so the order of the calls and the control bracket can be checked in one go.
    fn labels(log: &Mutex<Vec<AgentEvent>>) -> Vec<String> {
        log.lock()
            .unwrap()
            .iter()
            .map(|e| match e {
                AgentEvent::ToolCall { name, .. } => format!("call {name}"),
                AgentEvent::ToolResult { name, .. } => format!("result {name}"),
                AgentEvent::ApprovalRequest { .. } => "approval".to_string(),
                AgentEvent::AgentControl { active } => format!("control {active}"),
                other => format!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn agent_control_sits_between_an_approved_shell_call_and_its_result() {
        let tools = Recorder::default();
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        gate.set_auto_approve(true);
        let log = Mutex::new(Vec::new());
        let emit = |ev: AgentEvent| log.lock().unwrap().push(ev);
        let ctx = AgentCtx::new(&tools, &gate, &emit);
        assert!(ctx.call_tool("run_command", json!({"command": "pwd"})).is_ok());
        assert_eq!(labels(&log), ["call run_command", "control true", "control false", "result run_command"]);
    }

    #[test]
    fn agent_control_is_never_sent_for_read_tools_or_unapproved_calls() {
        let tools = Recorder::default();
        let gate = ApprovalGate::with_timeout(Duration::from_millis(50));
        let log = Mutex::new(Vec::new());
        let emit = |ev: AgentEvent| log.lock().unwrap().push(ev);
        let ctx = AgentCtx::new(&tools, &gate, &emit);
        assert!(ctx.read_tool("get_screen", json!({})).is_ok());
        assert!(ctx.call_tool("get_cwd", json!({})).is_ok());
        // Nobody answers: the approval times out and the call never reaches the tools.
        assert!(ctx.call_tool("run_command", json!({"command": "pwd"})).is_err());
        assert!(labels(&log).iter().all(|l| !l.starts_with("control")), "{:?}", labels(&log));
        assert_eq!(tools.reached(), vec!["get_screen".to_string(), "get_cwd".to_string()]);
    }

    /// "still running" only when the shell really is Running. Other timeouts name the state; not-run outcomes say why.
    #[test]
    fn run_command_outcomes_are_described_truthfully() {
        let text = |v: Value| outcome_text("sleep 30", &v);
        assert_eq!(
            text(json!({"outcome": "timed_out", "state": "running"})),
            "`sleep 30` is still running after the 30 s wait. It keeps running in the terminal; ask for the last command once it finishes."
        );
        let submitted = text(json!({"outcome": "timed_out", "state": "submitted"}));
        assert!(!submitted.contains("still running"), "{submitted}");
        assert!(submitted.contains("shell state: submitted"), "{submitted}");
        let abandoned = text(json!({"outcome": "not_run", "reason": "abandoned"}));
        assert!(abandoned.contains("did not run") && abandoned.contains("went back to the prompt"), "{abandoned}");
        let waiting = text(json!({"outcome": "not_run", "reason": "waiting_for_input"}));
        assert!(waiting.contains("waiting for the rest") && waiting.contains("Press Ctrl-C"), "{waiting}");
        assert!(!waiting.contains("still running"), "{waiting}");
        let cancelled = text(json!({"outcome": "not_run", "reason": "cancelled"}));
        assert!(cancelled.contains("you cancelled it") && cancelled.contains("keeps running"), "{cancelled}");
        assert!(!cancelled.contains("waiting for the rest"), "{cancelled}");
    }

    #[test]
    fn on_drop_runs_when_the_body_panics() {
        let ran = std::cell::Cell::new(false);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = OnDrop::new(|| ran.set(true));
            panic!("turn crashed");
        }));
        assert!(result.is_err());
        assert!(ran.get(), "the guard did not run while the panic unwound");
    }
}
